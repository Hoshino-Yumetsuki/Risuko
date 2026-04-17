//! Minimal BEP-5 DHT, scoped to what magnet resolution needs:
//! bootstrap, iterative `get_peers`, and a peer stream
//!
//! This is intentionally small compared to a full mainline DHT. It skips
//! routing-table persistence, `announce_peer`, token handling for
//! announcement, and IPv6. It is enough to discover peers for a magnet when
//! trackers are slow, partial, or absent

use std::collections::{BTreeMap, HashSet};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use rand::RngExt;
use tokio::net::{lookup_host, UdpSocket};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinSet;

use super::bencode::{decode_all, encode_to_vec, Value};
use super::core::Id20;

const K: usize = 8;
const ALPHA: usize = 3;
const QUERY_TIMEOUT: Duration = Duration::from_secs(4);
const MAX_ROUND_QUERIES: usize = 50;

pub const DEFAULT_BOOTSTRAP: &[&str] = &[
    "router.bittorrent.com:6881",
    "router.utorrent.com:6881",
    "dht.transmissionbt.com:6881",
    "dht.libtorrent.org:25401",
    "router.bitcomet.com:6881",
];

#[derive(Debug, Clone, Default)]
pub struct DhtConfig {
    pub bootstrap: Vec<String>,
    pub persistence_file: Option<std::path::PathBuf>,
}

/// A live DHT node. Holds a bound UDP socket and a background reader task
/// that routes responses to pending queries by transaction id
pub struct Dht {
    sock: Arc<UdpSocket>,
    our_id: Id20,
    bootstrap: Vec<String>,
    pending: Arc<Mutex<PendingMap>>,
}

type PendingMap = std::collections::HashMap<u16, oneshot::Sender<KrpcResponse>>;

struct KrpcResponse {
    from: SocketAddr,
    body: Value,
}

impl Dht {
    pub async fn spawn(config: DhtConfig) -> std::io::Result<Arc<Self>> {
        let sock = UdpSocket::bind("0.0.0.0:0").await?;
        let sock = Arc::new(sock);
        let our_id = random_id();
        let pending: Arc<Mutex<PendingMap>> = Arc::new(Mutex::new(Default::default()));

        let bootstrap = if config.bootstrap.is_empty() {
            DEFAULT_BOOTSTRAP.iter().map(|s| s.to_string()).collect()
        } else {
            config.bootstrap.clone()
        };

        let this = Arc::new(Self {
            sock: sock.clone(),
            our_id,
            bootstrap,
            pending: pending.clone(),
        });

        let reader_sock = sock.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 2048];
            loop {
                let (n, from) = match reader_sock.recv_from(&mut buf).await {
                    Ok(x) => x,
                    Err(_) => continue,
                };
                let Ok(msg) = decode_all(&buf[..n]) else {
                    continue;
                };
                let Some(ty) = msg.get(b"y").and_then(|v| v.as_bytes()) else {
                    continue;
                };
                if ty != b"r" && ty != b"e" {
                    continue;
                }
                let Some(tid) = msg.get(b"t").and_then(|v| v.as_bytes()) else {
                    continue;
                };
                let txn = match tid.len() {
                    2 => u16::from_be_bytes([tid[0], tid[1]]),
                    _ => continue,
                };
                let tx = pending.lock().remove(&txn);
                if let Some(tx) = tx {
                    let _ = tx.send(KrpcResponse { from, body: msg });
                }
            }
        });

        log::debug!(
            "DHT started: id={}, bootstrap={} nodes",
            hex::encode(our_id.as_bytes()),
            this.bootstrap.len()
        );
        Ok(this)
    }

    /// Start an iterative `get_peers` lookup and stream discovered peers on
    /// the returned channel until `budget` elapses (or the lookup converges
    /// with no further progress)
    pub fn get_peers_stream(
        self: &Arc<Self>,
        info_hash: Id20,
        budget: Duration,
    ) -> mpsc::UnboundedReceiver<SocketAddr> {
        let (tx, rx) = mpsc::unbounded_channel::<SocketAddr>();
        let this = self.clone();
        tokio::spawn(async move {
            let _ = tokio::time::timeout(budget, this.iterative_get_peers(info_hash, tx)).await;
        });
        rx
    }

    /// Synchronous-style collection: drains the stream for up to `budget`
    pub async fn get_peers(self: &Arc<Self>, info_hash: Id20) -> Vec<SocketAddr> {
        let mut rx = self.get_peers_stream(info_hash, Duration::from_secs(20));
        let mut out = Vec::new();
        while let Some(p) = rx.recv().await {
            out.push(p);
        }
        out
    }

    pub async fn announce_peer(&self, _info_hash: Id20, _port: u16) {
        // Omitted: we do not publish ourselves to the DHT. This keeps writes
        // off the wire and avoids the token-tracking machinery
    }

    async fn iterative_get_peers(
        self: Arc<Self>,
        info_hash: Id20,
        peer_tx: mpsc::UnboundedSender<SocketAddr>,
    ) {
        // Resolve bootstrap nodes to SocketAddrs concurrently
        let mut addrs: Vec<SocketAddr> = Vec::new();
        for host in &self.bootstrap {
            if let Ok(iter) = lookup_host(host).await {
                addrs.extend(iter);
            }
        }
        if addrs.is_empty() {
            log::debug!("dht: no bootstrap nodes resolved");
            return;
        }

        // BTreeMap keyed by XOR distance to info_hash → candidate endpoints
        // We keep the K closest "live" nodes we've heard responses from
        let mut shortlist: BTreeMap<Id20, SocketAddr> = BTreeMap::new();
        let mut queried: HashSet<SocketAddr> = HashSet::new();
        let mut peers_seen: HashSet<SocketAddr> = HashSet::new();

        // Seed: ask bootstrap nodes with a dummy id = info_hash (so their
        // responses contain nodes close to the target)
        for a in addrs.iter().take(MAX_ROUND_QUERIES) {
            queried.insert(*a);
        }

        let mut futs: JoinSet<Option<(SocketAddr, Vec<SocketAddr>, Vec<(Id20, SocketAddr)>)>> =
            JoinSet::new();
        for a in addrs {
            let this = self.clone();
            futs.spawn(async move { this.query_get_peers(a, info_hash).await });
        }

        let mut total_peers = 0usize;
        let mut total_nodes = 0usize;
        let mut rounds_without_progress = 0usize;

        loop {
            let Some(joined) = futs.join_next().await else {
                break;
            };
            let res = joined.ok().flatten();

            let Some((from, peers, nodes)) = res else {
                continue;
            };

            // Emit peers immediately
            let mut new_peers = 0usize;
            for p in peers {
                if peers_seen.insert(p) {
                    new_peers += 1;
                    if peer_tx.send(p).is_err() {
                        return;
                    }
                }
            }
            total_peers += new_peers;

            // Record the responder as a live DHT node (we need its distance)
            // We don't know its id unless it returned nodes containing itself,
            // but we keep it in the shortlist keyed by distance to info_hash
            // using a hash of its socket addr as a pseudo-id. This is only
            // used for ordering visits, which is fine
            let pseudo = pseudo_id(from);
            shortlist.insert(xor(&pseudo, &info_hash), from);

            // Merge any learned nodes into the shortlist
            let mut progressed = false;
            for (nid, naddr) in &nodes {
                total_nodes += 1;
                let d = xor(nid, &info_hash);
                if let std::collections::btree_map::Entry::Vacant(e) = shortlist.entry(d) {
                    e.insert(*naddr);
                    progressed = true;
                }
            }

            // Trim to K * 2 to keep memory bounded
            while shortlist.len() > K * 4 {
                shortlist.pop_last();
            }

            if progressed || new_peers > 0 {
                rounds_without_progress = 0;
            } else {
                rounds_without_progress += 1;
            }

            // Queue up to ALPHA new queries from the closest unvisited
            let mut dispatched = 0usize;
            let mut to_dispatch: Vec<SocketAddr> = Vec::new();
            for (_d, a) in shortlist.iter().take(K * 2) {
                if queried.insert(*a) {
                    to_dispatch.push(*a);
                    dispatched += 1;
                    if dispatched >= ALPHA {
                        break;
                    }
                }
            }
            for a in to_dispatch {
                let this = self.clone();
                futs.spawn(async move { this.query_get_peers(a, info_hash).await });
            }

            if dispatched == 0 && futs.is_empty() {
                break;
            }
            if rounds_without_progress > 20 && peers_seen.len() >= 40 {
                break;
            }
        }

        log::debug!(
            "dht get_peers: peers={} nodes_learned={} nodes_queried={}",
            total_peers,
            total_nodes,
            queried.len()
        );
    }

    async fn query_get_peers(
        self: Arc<Self>,
        target: SocketAddr,
        info_hash: Id20,
    ) -> Option<(SocketAddr, Vec<SocketAddr>, Vec<(Id20, SocketAddr)>)> {
        let (txn, rx) = self.register_transaction();
        let packet = build_get_peers(txn, &self.our_id, &info_hash);
        if self.sock.send_to(&packet, target).await.is_err() {
            self.pending.lock().remove(&txn);
            return None;
        }
        let resp = match tokio::time::timeout(QUERY_TIMEOUT, rx).await {
            Ok(Ok(r)) => r,
            _ => {
                self.pending.lock().remove(&txn);
                return None;
            }
        };
        parse_get_peers_response(&resp.body).map(|(peers, nodes)| (resp.from, peers, nodes))
    }

    fn register_transaction(&self) -> (u16, oneshot::Receiver<KrpcResponse>) {
        let (tx, rx) = oneshot::channel();
        let mut map = self.pending.lock();
        let mut txn: u16 = rand::rng().random();
        while map.contains_key(&txn) {
            txn = txn.wrapping_add(1);
        }
        map.insert(txn, tx);
        (txn, rx)
    }
}

fn random_id() -> Id20 {
    let mut b = [0u8; 20];
    rand::rng().fill(&mut b[..]);
    Id20::from_slice(&b).expect("20 bytes")
}

fn pseudo_id(addr: SocketAddr) -> Id20 {
    // Stable per-address id for shortlist ordering. Not a real DHT id
    use sha1::{Digest, Sha1};
    let mut h = Sha1::new();
    match addr {
        SocketAddr::V4(a) => {
            h.update(a.ip().octets());
            h.update(a.port().to_be_bytes());
        }
        SocketAddr::V6(a) => {
            h.update(a.ip().octets());
            h.update(a.port().to_be_bytes());
        }
    }
    Id20::from_slice(&h.finalize()[..20]).unwrap()
}

fn xor(a: &Id20, b: &Id20) -> Id20 {
    let mut out = [0u8; 20];
    for i in 0..20 {
        out[i] = a.as_bytes()[i] ^ b.as_bytes()[i];
    }
    Id20::from_slice(&out).unwrap()
}

fn build_get_peers(txn: u16, our_id: &Id20, info_hash: &Id20) -> Vec<u8> {
    let args = Value::Dict(vec![
        (b"id".to_vec(), Value::Bytes(our_id.as_bytes().to_vec())),
        (
            b"info_hash".to_vec(),
            Value::Bytes(info_hash.as_bytes().to_vec()),
        ),
    ]);
    let tid = txn.to_be_bytes().to_vec();
    let msg = Value::Dict(vec![
        (b"a".to_vec(), args),
        (b"q".to_vec(), Value::Bytes(b"get_peers".to_vec())),
        (b"t".to_vec(), Value::Bytes(tid)),
        (b"y".to_vec(), Value::Bytes(b"q".to_vec())),
    ]);
    encode_to_vec(&msg)
}

fn parse_get_peers_response(body: &Value) -> Option<(Vec<SocketAddr>, Vec<(Id20, SocketAddr)>)> {
    let r = body.get(b"r")?.as_dict()?;
    let r_val = Value::Dict(r.to_vec());
    let mut peers: Vec<SocketAddr> = Vec::new();
    if let Some(values) = r_val.get(b"values").and_then(|v| v.as_list()) {
        for v in values {
            if let Some(b) = v.as_bytes() {
                if b.len() == 6 {
                    let ip = Ipv4Addr::new(b[0], b[1], b[2], b[3]);
                    let port = u16::from_be_bytes([b[4], b[5]]);
                    peers.push(SocketAddr::V4(SocketAddrV4::new(ip, port)));
                }
            }
        }
    }
    let mut nodes: Vec<(Id20, SocketAddr)> = Vec::new();
    if let Some(n) = r_val.get(b"nodes").and_then(|v| v.as_bytes()) {
        // Each compact node: 20 bytes id + 4 bytes ipv4 + 2 bytes port
        for chunk in n.chunks_exact(26) {
            let id = Id20::from_slice(&chunk[..20]).ok()?;
            let ip = Ipv4Addr::new(chunk[20], chunk[21], chunk[22], chunk[23]);
            let port = u16::from_be_bytes([chunk[24], chunk[25]]);
            nodes.push((id, SocketAddr::V4(SocketAddrV4::new(ip, port))));
        }
    }
    Some((peers, nodes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xor_is_commutative_and_reflexive() {
        let a = Id20::from_slice(&[0x11u8; 20]).unwrap();
        let b = Id20::from_slice(&[0x22u8; 20]).unwrap();
        assert_eq!(xor(&a, &b), xor(&b, &a));
        assert_eq!(xor(&a, &a).as_bytes(), &[0u8; 20]);
    }

    #[test]
    fn get_peers_packet_is_bencoded_krpc_query() {
        let our_id = Id20::from_slice(&[0u8; 20]).unwrap();
        let info_hash = Id20::from_slice(&[1u8; 20]).unwrap();
        let packet = build_get_peers(0xBEEF, &our_id, &info_hash);
        let decoded = decode_all(&packet).unwrap();
        assert_eq!(
            decoded.get(b"q").and_then(|v| v.as_bytes()),
            Some(b"get_peers" as &[u8])
        );
        assert_eq!(
            decoded.get(b"y").and_then(|v| v.as_bytes()),
            Some(b"q" as &[u8])
        );
        assert_eq!(
            decoded.get(b"t").and_then(|v| v.as_bytes()),
            Some(&[0xBE, 0xEF][..])
        );
        let a = decoded.get(b"a").unwrap().as_dict().unwrap();
        assert_eq!(a.len(), 2);
    }

    #[test]
    fn parse_response_extracts_peers_and_nodes() {
        // values: [6-byte peer for 1.2.3.4:5678]
        // nodes: 26 bytes (id=0x22... ip=9.8.7.6 port=11111)
        let peer_bytes: Vec<u8> = vec![1, 2, 3, 4, (5678u16 >> 8) as u8, (5678u16 & 0xff) as u8];
        let mut node_bytes = vec![0x22u8; 20];
        node_bytes.extend_from_slice(&[9, 8, 7, 6]);
        node_bytes.extend_from_slice(&11111u16.to_be_bytes());

        let r = Value::Dict(vec![
            (b"id".to_vec(), Value::Bytes(vec![0u8; 20])),
            (b"nodes".to_vec(), Value::Bytes(node_bytes)),
            (
                b"values".to_vec(),
                Value::List(vec![Value::Bytes(peer_bytes)]),
            ),
        ]);
        let body = Value::Dict(vec![
            (b"r".to_vec(), r),
            (b"t".to_vec(), Value::Bytes(b"aa".to_vec())),
            (b"y".to_vec(), Value::Bytes(b"r".to_vec())),
        ]);
        let (peers, nodes) = parse_get_peers_response(&body).unwrap();
        assert_eq!(peers.len(), 1);
        assert_eq!(
            peers[0],
            SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(1, 2, 3, 4), 5678))
        );
        assert_eq!(nodes.len(), 1);
        assert_eq!(
            nodes[0].1,
            SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(9, 8, 7, 6), 11111))
        );
    }
}
