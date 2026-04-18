//! Magnet URI → info-dict resolution
//!
//! Discovers peers via user-supplied trackers and downloads the `info` dict
//! from them using BEP-9 (ut_metadata).  DHT is not used (the in-tree DHT is
//! a stub)

use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use sha1::{Digest, Sha1};
use tokio::sync::{mpsc, oneshot, Semaphore};
use tokio::task::JoinSet;

use super::core::{generate_peer_id, Id20, Magnet};
use super::dht::{Dht, DhtConfig};
use super::peer::{connect, PeerCommand, PeerEvent, SpawnPeer};
use super::tracker::{announce, AnnounceEvent, AnnounceRequest};
use super::wire::extended::{
    parse_ut_metadata, ut_metadata_request, ut_metadata_type, ExtHandshake, EXT_HANDSHAKE_ID,
};
use super::wire::Message;

const META_PIECE_SIZE: usize = 16 * 1024;
const MAX_METADATA_SIZE: usize = 32 * 1024 * 1024;
const OUR_UT_METADATA_ID: u8 = 3;
const OUR_UT_PEX_ID: u8 = 4;
const TRACKER_TIMEOUT: Duration = Duration::from_secs(10);
const PEER_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const PEER_READ_TIMEOUT: Duration = Duration::from_secs(10);
const PEER_TOTAL_TIMEOUT: Duration = Duration::from_secs(12);
const MAX_CONCURRENT_PEERS: usize = 128;

/// Resolution result: the info-hash, raw bencoded info dict, and the
/// union of trackers (magnet's `tr=` + caller-supplied)
pub struct Resolved {
    pub info_hash: Id20,
    pub info_bytes: Vec<u8>,
    pub trackers: Vec<String>,
    pub display_name: Option<String>,
}

/// Resolve a magnet URI to its raw info dict.
pub async fn resolve(
    magnet_uri: &str,
    extra_trackers: &[String],
    budget: Duration,
    encryption: crate::peer::EncryptionPolicy,
) -> Result<Resolved, String> {
    let magnet = Magnet::parse(magnet_uri).map_err(|e| e.to_string())?;
    let info_hash = magnet.info_hash();

    let mut trackers: Vec<String> = magnet.trackers.clone();
    for t in extra_trackers {
        if !trackers.iter().any(|x| x == t) {
            trackers.push(t.clone());
        }
    }

    let our_peer_id = generate_peer_id();
    let req = AnnounceRequest {
        info_hash,
        peer_id: our_peer_id,
        port: 6881,
        uploaded: 0,
        downloaded: 0,
        left: 0,
        event: AnnounceEvent::Started,
        num_want: 200,
    };

    let deadline = Instant::now() + budget;
    let started = Instant::now();

    // Peer addresses discovered by trackers stream through this channel
    // Unbounded because trackers return bursty batches but we drain greedily
    let (peer_tx, mut peer_rx) = mpsc::unbounded_channel::<SocketAddr>();

    // Fire off all tracker announces in parallel. Each feeds peer_tx and has
    // its own bounded timeout, so a slow tracker never gates the others
    let mut tracker_set: JoinSet<()> = JoinSet::new();
    for url in trackers.clone() {
        let req = req.clone();
        let tx = peer_tx.clone();
        let per_tracker = budget.min(TRACKER_TIMEOUT);
        tracker_set.spawn(async move {
            match announce(&url, &req, per_tracker).await {
                Ok(r) => {
                    log::debug!("tracker {url} returned {} peers", r.peers.len());
                    for p in r.peers {
                        let _ = tx.send(p);
                    }
                }
                Err(e) => log::debug!("tracker {url} failed: {e}"),
            }
        });
    }

    // Fire up DHT in parallel; it feeds the same peer channel as trackers
    // If DHT fails to start (firewalled UDP, etc.) we just lose that source
    let dht_handle: Option<tokio::task::JoinHandle<()>> =
        match Dht::spawn(DhtConfig::default()).await {
            Ok(dht) => {
                let tx = peer_tx.clone();
                let dht_budget = budget.min(Duration::from_secs(60));
                let mut dht_rx = dht.get_peers_stream(info_hash, dht_budget);
                Some(tokio::spawn(async move {
                    while let Some(p) = dht_rx.recv().await {
                        if tx.send(p).is_err() {
                            break;
                        }
                    }
                }))
            }
            Err(e) => {
                log::debug!("dht spawn failed: {e}");
                None
            }
        };
    drop(peer_tx);

    // First successful metadata download wins via this oneshot
    let (result_tx, result_rx) = oneshot::channel::<Vec<u8>>();
    let result_tx: Arc<Mutex<Option<oneshot::Sender<Vec<u8>>>>> =
        Arc::new(Mutex::new(Some(result_tx)));

    // Bound fan-out so we don't open thousands of sockets
    let sem = Arc::new(Semaphore::new(MAX_CONCURRENT_PEERS));

    // Driver: consume peer addresses and spawn bounded fetch tasks
    let driver = {
        let result_tx = result_tx.clone();
        let sem = sem.clone();
        async move {
            let mut seen: HashSet<SocketAddr> = HashSet::new();
            let mut joinset: JoinSet<()> = JoinSet::new();
            let mut attempted = 0usize;

            loop {
                // If a winner already published, stop spawning
                if result_tx.lock().is_none() {
                    break;
                }
                tokio::select! {
                    maybe = peer_rx.recv() => {
                        let Some(addr) = maybe else {
                            // Trackers drained; wait for in-flight to finish
                            while joinset.join_next().await.is_some() {
                                if result_tx.lock().is_none() { break; }
                            }
                            break;
                        };
                        if !seen.insert(addr) { continue; }
                        attempted += 1;

                        let permit = match Arc::clone(&sem).acquire_owned().await {
                            Ok(p) => p,
                            Err(_) => break,
                        };
                        let result_tx = result_tx.clone();
                        joinset.spawn(async move {
                            let _permit = permit;
                            let fetched = tokio::time::timeout(
                                PEER_TOTAL_TIMEOUT,
                                try_fetch_from_peer(addr, info_hash, our_peer_id, encryption),
                            )
                            .await
                            .ok()
                            .flatten();
                            let Some(bytes) = fetched else { return };

                            let digest = Sha1::digest(&bytes);
                            let ok = Id20::from_slice(digest.as_slice())
                                .map(|h| h == info_hash)
                                .unwrap_or(false);
                            if !ok {
                                log::debug!("peer {addr}: info hash mismatch");
                                return;
                            }
                            if let Some(tx) = result_tx.lock().take() {
                                let _ = tx.send(bytes);
                            }
                        });
                    }
                    Some(_done) = joinset.join_next(), if !joinset.is_empty() => {
                        // Reap completed tasks; slot is implicitly freed by permit drop
                    }
                }
            }
            attempted
        }
    };

    // Race the driver against the overall deadline and the oneshot winner
    let overall = deadline.saturating_duration_since(Instant::now());
    let winner = tokio::select! {
        biased;
        got = result_rx => got.ok(),
        _attempted = driver => None,
        _ = tokio::time::sleep(overall) => None,
    };

    tracker_set.abort_all();
    if let Some(h) = dht_handle {
        h.abort();
    }

    match winner {
        Some(info_bytes) => {
            log::info!("Resolved magnet in {:?}", started.elapsed());
            Ok(Resolved {
                info_hash,
                info_bytes,
                trackers,
                display_name: magnet.display_name.clone(),
            })
        }
        None => Err("failed to fetch metadata from any peer".into()),
    }
}

async fn try_fetch_from_peer(
    addr: SocketAddr,
    info_hash: Id20,
    our_peer_id: Id20,
    encryption: crate::peer::EncryptionPolicy,
) -> Option<Vec<u8>> {
    let (handle, rx) = connect(SpawnPeer {
        addr,
        info_hash,
        our_peer_id,
        connect_timeout: PEER_CONNECT_TIMEOUT,
        read_timeout: PEER_READ_TIMEOUT,
        encryption,
    })
    .await
    .ok()?;

    // Run the protocol inside a helper so every exit path disconnects the
    // peer actor below. Otherwise timed-out or rejected probes leak the
    // socket and reader task.
    let result = try_fetch_from_peer_inner(&handle, rx).await;
    let _ = handle.tx.send(PeerCommand::Disconnect).await;
    result
}

async fn try_fetch_from_peer_inner(
    handle: &super::peer::PeerHandle,
    mut rx: tokio::sync::mpsc::Receiver<PeerEvent>,
) -> Option<Vec<u8>> {
    // Pipeline our extended handshake immediately; we'll validate that the
    // peer actually supports extensions when we see their Handshook event
    // This saves one async round-trip per peer
    let our_hs = ExtHandshake::new_outgoing(OUR_UT_METADATA_ID, OUR_UT_PEX_ID, None);
    handle
        .tx
        .send(PeerCommand::Send(Message::Extended {
            ext_id: EXT_HANDSHAKE_ID,
            payload: our_hs.encode(),
        }))
        .await
        .ok()?;

    // Collect Handshook and the peer's extended handshake from a single
    // receive loop. The peer's extended handshake message can arrive before
    // the BT handshake event under some orderings; draining two sequential
    // loops would drop whichever arrives first in the other arm.
    let mut peer_supports_ext: Option<bool> = None;
    let peer_ext = loop {
        match rx.recv().await? {
            PeerEvent::Handshook { reserved, .. } => {
                let supports = reserved[5] & 0x10 != 0;
                if !supports {
                    return None;
                }
                peer_supports_ext = Some(true);
            }
            PeerEvent::Message(Message::Extended { ext_id: 0, payload }) => {
                let Some(h) = ExtHandshake::decode(&payload) else {
                    return None;
                };
                if peer_supports_ext.is_none() {
                    // Extended handshake must be preceded by Handshook with
                    // the extension bit set. Keep looping until Handshook
                    // confirms support, but stash the decoded dict.
                    match wait_for_handshook(&mut rx).await {
                        Some(true) => break h,
                        _ => return None,
                    }
                }
                break h;
            }
            PeerEvent::Disconnected { .. } => return None,
            _ => continue,
        }
    };

    let their_ut_metadata_id = peer_ext.ut_metadata_id()?;
    let total_size = peer_ext.metadata_size? as usize;
    if total_size == 0 || total_size > MAX_METADATA_SIZE {
        return None;
    }
    let num_pieces = total_size.div_ceil(META_PIECE_SIZE);
    let mut pieces: Vec<Option<Vec<u8>>> = vec![None; num_pieces];

    for i in 0..num_pieces {
        let payload = ut_metadata_request(i as i64);
        handle
            .tx
            .send(PeerCommand::Send(Message::Extended {
                ext_id: their_ut_metadata_id,
                payload,
            }))
            .await
            .ok()?;
    }

    let mut remaining = num_pieces;
    while remaining > 0 {
        match rx.recv().await? {
            PeerEvent::Message(Message::Extended { ext_id, payload })
                if ext_id == OUR_UT_METADATA_ID =>
            {
                let msg = parse_ut_metadata(payload)?;
                if msg.msg_type == ut_metadata_type::DATA {
                    let idx = msg.piece as usize;
                    if idx < num_pieces && pieces[idx].is_none() {
                        let expected = if idx + 1 == num_pieces {
                            total_size - idx * META_PIECE_SIZE
                        } else {
                            META_PIECE_SIZE
                        };
                        if msg.block.len() == expected {
                            pieces[idx] = Some(msg.block.to_vec());
                            remaining -= 1;
                        }
                    }
                } else if msg.msg_type == ut_metadata_type::REJECT {
                    return None;
                }
            }
            PeerEvent::Disconnected { .. } => return None,
            _ => continue,
        }
    }

    let mut out = Vec::with_capacity(total_size);
    for p in pieces {
        out.extend_from_slice(&p?);
    }
    if out.len() != total_size {
        return None;
    }

    Some(out)
}

async fn wait_for_handshook(rx: &mut tokio::sync::mpsc::Receiver<PeerEvent>) -> Option<bool> {
    loop {
        match rx.recv().await? {
            PeerEvent::Handshook { reserved, .. } => {
                return Some(reserved[5] & 0x10 != 0);
            }
            PeerEvent::Disconnected { .. } => return None,
            _ => continue,
        }
    }
}

/// Build a minimal `.torrent` blob from a raw info dict plus optional
/// trackers, suitable for feeding back into [`crate::parse_torrent`]
pub fn synth_torrent_bytes(info_bytes: &[u8], trackers: &[String]) -> Vec<u8> {
    // bencode: d [announce-list] 4:info <info_bytes> e
    let mut out = Vec::with_capacity(info_bytes.len() + 64);
    out.push(b'd');
    if !trackers.is_empty() {
        // announce: use first as primary
        let primary = trackers[0].as_bytes();
        out.extend_from_slice(b"8:announce");
        out.extend_from_slice(format!("{}:", primary.len()).as_bytes());
        out.extend_from_slice(primary);
        // announce-list: list of tiers, each a list of URLs
        out.extend_from_slice(b"13:announce-listl");
        for t in trackers {
            out.push(b'l');
            let b = t.as_bytes();
            out.extend_from_slice(format!("{}:", b.len()).as_bytes());
            out.extend_from_slice(b);
            out.push(b'e');
        }
        out.push(b'e');
    }
    out.extend_from_slice(b"4:info");
    out.extend_from_slice(info_bytes);
    out.push(b'e');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synth_torrent_round_trips_through_parse() {
        use crate::bencode::{encode_to_vec, Value};
        let pieces = vec![0u8; 20];
        let info = Value::Dict(vec![
            (b"length".to_vec(), Value::Int(1024)),
            (b"name".to_vec(), Value::Bytes(b"hello".to_vec())),
            (b"piece length".to_vec(), Value::Int(1024)),
            (b"pieces".to_vec(), Value::Bytes(pieces)),
        ]);
        let info_bytes = encode_to_vec(&info);
        let trackers = vec!["http://tracker.example/announce".to_string()];
        let torrent_bytes = synth_torrent_bytes(&info_bytes, &trackers);
        let meta = crate::parse_torrent(&torrent_bytes).unwrap();
        assert_eq!(meta.info.name, "hello");
        assert_eq!(meta.info.total_length(), 1024);
        assert_eq!(
            meta.announce.as_deref(),
            Some("http://tracker.example/announce")
        );
    }
}
