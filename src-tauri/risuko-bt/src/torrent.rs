//! Per-torrent state machine

pub mod stats;

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use arc_swap::ArcSwapOption;
use bytes::Bytes;
use parking_lot::Mutex;
use sha1::{Digest, Sha1};
use tokio::sync::{mpsc, oneshot};
use tokio::time::{interval, MissedTickBehavior};

use super::core::{Id20, Lengths, TorrentMeta, ValidatedTorrentMetaV1Info};
use super::peer::{connect, PeerCommand, PeerEvent, SpawnPeer};
use super::piece::{ChunkTracker, PieceTracker};
use super::storage::{FilesystemStorage, StorageBackend};
use super::tracker::{announce as tracker_announce, AnnounceEvent, AnnounceRequest};
use super::wire::Message;

pub use stats::{
    AggregatedLiveStats, LiveStats, PeerSnapshot, Snapshot, SpeedSample, TorrentStats,
};

/// Default maximum concurrent 16 KiB chunk requests per peer. BitTorrent
/// clients typically pipeline 32-128 (libtorrent `reqq`). Too low here caps
/// per-peer throughput at `MAX_OUTSTANDING * 16 KiB / round-trip`
const DEFAULT_MAX_OUTSTANDING_PER_PEER: usize = 128;
/// Default cap on total concurrent peers held by the torrent state machine
const DEFAULT_MAX_PEERS: usize = 100;
/// Cap on outbound dials whose handshake hasn't completed. Without a separate
/// budget, a swarm where many peers are unreachable will park every slot in
/// `pending_dials` for the connect timeout and starve real connections.
const MAX_PENDING_DIALS: usize = 256;
/// Time after which an outstanding chunk request to a peer is considered
/// stale and reclaimed.
/// If omitted, TCP-alive-but-stalled peers progressively hoard pieces until
/// download speed collapses even while peer count stays high (each slow
/// peer permanently marks its pieces `in_flight`, excluding them from
/// `choose_requestable_piece`).
///
/// 8 s is comfortably above any realistic 16 KiB chunk RTT (a 16 Kbps link
/// still delivers in ~8 s) while draining slow peers ~2.5× faster than the
/// previous 20 s value. Combined with endgame duplication + Cancel, this
/// keeps pipeline utilisation high all the way through the last 1 %
const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);

pub struct TorrentInit {
    pub meta: TorrentMeta,
    pub lengths: Lengths,
    pub root_dir: PathBuf,
    pub only_files: Option<Vec<usize>>,
    pub max_outstanding_per_peer: Option<usize>,
    pub max_peers: Option<usize>,
    pub encryption: super::peer::EncryptionPolicy,
}

#[derive(Debug)]
pub enum TorrentCommand {
    AddPeer(SocketAddr),
    AddInboundPeer {
        addr: SocketAddr,
        cmd_tx: mpsc::Sender<PeerCommand>,
        event_rx: mpsc::Receiver<PeerEvent>,
    },
    Pause(oneshot::Sender<()>),
    Unpause(oneshot::Sender<()>),
    Stop(oneshot::Sender<()>),
}

pub struct ManagedTorrent {
    pub id: usize,
    pub info_hash: Id20,
    pub name: Option<String>,
    pub metadata: ArcSwapOption<TorrentMeta>,
    pub(crate) cmd_tx: mpsc::Sender<TorrentCommand>,
    pub(crate) stats: Arc<Mutex<TorrentStats>>,
}

impl ManagedTorrent {
    pub fn info_hash(&self) -> Id20 {
        self.info_hash
    }
    pub fn name(&self) -> Option<String> {
        self.name.clone()
    }
    pub fn stats(&self) -> TorrentStats {
        let mut s = self.stats.lock().clone();
        s.refresh_live();
        s
    }
    pub fn with_metadata<T>(&self, f: impl FnOnce(&TorrentMeta) -> T) -> Result<T, &'static str> {
        match self.metadata.load().as_ref() {
            Some(m) => Ok(f(m.as_ref())),
            None => Err("metadata not yet available"),
        }
    }
    pub(crate) fn cmd_tx(&self) -> mpsc::Sender<TorrentCommand> {
        self.cmd_tx.clone()
    }
}

pub async fn spawn(
    id: usize,
    init: TorrentInit,
    our_peer_id: Id20,
    listen_port: u16,
) -> std::io::Result<Arc<ManagedTorrent>> {
    let info_hash = init.meta.info_hash;
    let name = Some(init.meta.info.name.clone());
    let (cmd_tx, cmd_rx) = mpsc::channel::<TorrentCommand>(64);
    let file_lens: Vec<u64> = init.meta.info.iter_file_details().map(|f| f.len).collect();
    let stats = Arc::new(Mutex::new(TorrentStats::initial(
        init.lengths.total_length(),
        file_lens,
    )));
    let meta_arc = Arc::new(init.meta.clone());
    let metadata_swap = ArcSwapOption::new(Some(meta_arc));
    let handle = Arc::new(ManagedTorrent {
        id,
        info_hash,
        name,
        metadata: metadata_swap,
        cmd_tx,
        stats: stats.clone(),
    });
    tokio::spawn(torrent_loop(
        id,
        init,
        our_peer_id,
        listen_port,
        cmd_rx,
        stats,
    ));
    Ok(handle)
}

struct Peer {
    addr: SocketAddr,
    cmd_tx: mpsc::Sender<PeerCommand>,
    bitfield: Vec<u8>,
    am_choking: bool,
    am_interested: bool,
    peer_choking: bool,
    peer_interested: bool,
    outstanding: Vec<(u32, u32, u32)>,
}

/// Result of an off-runtime piece hash verification
struct VerifyResult {
    piece_index: u32,
    hash: [u8; 20],
    /// Set when the disk write for this piece returned an error. The
    /// torrent loop must not mark the piece local in that case — the data
    /// on disk is incomplete and the piece must be re-requested
    write_failed: bool,
}

/// In-memory accumulator for chunks of an in-flight piece. Keeping the
/// piece buffer here lets us:
///  - Skip the disk write per chunk (we write the full piece once on
///    completion, off the main loop)
///  - Skip the disk read-back for SHA1 verification (hash from RAM)
///
/// Memory bound: at most `max_peers` pieces in flight (~ piece_length each)
struct PieceAssembly {
    buf: Vec<u8>,
    /// Set of chunk indices already written into `buf`. Used to ignore
    /// duplicate chunks (which arrive in endgame mode when the same chunk
    /// is requested from multiple peers) without double-counting bytes
    received_chunks: HashSet<u32>,
    received_bytes: u32,
    expected_bytes: u32,
    /// Set once the piece has been handed off to the verify/write task.
    /// Late-arriving duplicates after this point must not recreate or
    /// mutate the assembly
    completed: bool,
}

async fn torrent_loop(
    torrent_id: usize,
    init: TorrentInit,
    our_peer_id: Id20,
    listen_port: u16,
    mut cmd_rx: mpsc::Receiver<TorrentCommand>,
    stats: Arc<Mutex<TorrentStats>>,
) {
    let info = Arc::new(init.meta.info.clone());
    let info_hash = init.meta.info_hash;
    let lengths = init.lengths;
    let encryption = init.encryption;
    let max_outstanding = init
        .max_outstanding_per_peer
        .unwrap_or(DEFAULT_MAX_OUTSTANDING_PER_PEER)
        .max(1);
    let max_peers = init.max_peers.unwrap_or(DEFAULT_MAX_PEERS).max(1);
    let storage = Arc::new(FilesystemStorage::new(&info, &init.root_dir));
    if let Err(e) = storage.preallocate().await {
        log::warn!("preallocate failed for {info_hash}: {e}");
    }
    let mut piece_tracker = PieceTracker::new(lengths);
    let mut chunk_tracker = ChunkTracker::new(lengths);
    let mut piece_assemblies: HashMap<u32, PieceAssembly> = HashMap::new();
    scan_existing_pieces(&info, &storage, &lengths, &mut piece_tracker).await;
    {
        let mut s = stats.lock();
        s.progress_bytes = completed_bytes(&piece_tracker, &lengths);
        s.file_progress = compute_file_progress(&piece_tracker, &lengths, storage.layout());
        s.finished = piece_tracker.is_complete();
    }

    let mut peer_addr_rx = spawn_tracker_pollers(
        collect_trackers(&init.meta),
        info_hash,
        our_peer_id,
        listen_port,
        lengths.total_length(),
    );

    // Large enough to not block peers: with MAX_PEERS peers each potentially
    // delivering MAX_OUTSTANDING_PER_PEER Piece events in rapid succession,
    // undersizing this channel serializes the entire download
    let (peer_event_tx, mut peer_event_rx) = mpsc::channel::<(u32, PeerEvent)>(8192);
    // Piece hash results come back asynchronously so the main loop never
    // blocks on SHA1 verification. Without this, a single 4 MB piece hash
    // costs ~10 ms of CPU during which no peer event can be serviced
    let (verify_tx, mut verify_rx) = mpsc::channel::<VerifyResult>(256);
    let mut peers: HashMap<u32, Peer> = HashMap::new();
    let mut next_pid: u32 = 1;
    let mut known_addrs: HashSet<SocketAddr> = HashSet::new();
    // Outbound dials that have been spawned but whose peer has not completed
    // the BT handshake yet. Tracked separately so the max-peer cap accounts
    // for in-flight connection bursts, not just handshook peers.
    let mut pending_dials: HashMap<u32, SocketAddr> = HashMap::new();
    let mut paused = false;
    let mut tick = interval(Duration::from_millis(500));
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut last_tick = Instant::now();
    let mut bytes_this_tick = (0u64, 0u64);
    // Upload bytes accumulate from spawned send tasks; share via atomic so
    // we only credit them after the disk read and channel send succeed
    let upload_tick: Arc<AtomicU64> = Arc::new(AtomicU64::new(0));
    // Track in-flight piece write/hash tasks so Stop can wait for (or
    // cancel) them. Without this, a Stop racing with a piece-completion
    // write_at could leave a partially-written piece on disk while the
    // torrent loop has already returned
    let mut write_tasks: tokio::task::JoinSet<()> = tokio::task::JoinSet::new();

    loop {
        tokio::select! {
            Some(cmd) = cmd_rx.recv() => match cmd {
                TorrentCommand::AddPeer(addr) => {
                    if !paused
                        && known_addrs.insert(addr)
                        && peers.len() < max_peers
                        && pending_dials.len() < MAX_PENDING_DIALS
                    {
                        let pid = next_pid; next_pid += 1;
                        pending_dials.insert(pid, addr);
                        spawn_outbound_peer(torrent_id, pid, addr, info_hash, our_peer_id, peer_event_tx.clone(), encryption);
                    }
                }
                TorrentCommand::AddInboundPeer { addr, cmd_tx, event_rx } => {
                    if !paused
                        && known_addrs.insert(addr)
                        && peers.len() < max_peers
                    {
                        let pid = next_pid; next_pid += 1;
                        adopt_inbound_peer(pid, addr, cmd_tx, event_rx, peer_event_tx.clone(), &mut peers, &lengths, &mut piece_tracker).await;
                    }
                }
                TorrentCommand::Pause(ack) => {
                    paused = true;
                    for (_, p) in peers.drain() {
                        let _ = p.cmd_tx.send(PeerCommand::Disconnect).await;
                    }
                    pending_dials.clear();
                    known_addrs.clear();
                    let _ = ack.send(());
                }
                TorrentCommand::Unpause(ack) => {
                    paused = false;
                    let _ = ack.send(());
                }
                TorrentCommand::Stop(ack) => {
                    for (_, p) in peers.drain() {
                        let _ = p.cmd_tx.send(PeerCommand::Disconnect).await;
                    }
                    pending_dials.clear();
                    // Wait for any in-flight piece write/verify tasks so we
                    // don't return Stop while a write_at is still pending.
                    // shutdown() aborts then joins all handles
                    write_tasks.shutdown().await;
                    let _ = ack.send(());
                    break;
                }
            },
            Some(addr) = peer_addr_rx.recv() => {
                if !paused
                    && known_addrs.insert(addr)
                    && peers.len() < max_peers
                    && pending_dials.len() < MAX_PENDING_DIALS
                {
                    let pid = next_pid; next_pid += 1;
                    pending_dials.insert(pid, addr);
                    spawn_outbound_peer(torrent_id, pid, addr, info_hash, our_peer_id, peer_event_tx.clone(), encryption);
                }
            }
            Some((pid, ev)) = peer_event_rx.recv() => {
                let kick = process_peer_event(
                    torrent_id, pid, ev, &mut peers, &mut piece_tracker, &mut chunk_tracker,
                    &mut piece_assemblies,
                    &lengths, &storage, &stats, &mut bytes_this_tick,
                    &upload_tick,
                    &mut write_tasks,
                    &mut pending_dials, &mut known_addrs,
                    &verify_tx,
                    max_peers,
                ).await;
                if kick && !paused {
                    drive_peer(pid, &mut peers, &mut piece_tracker, &mut chunk_tracker, max_outstanding).await;
                }
            }
            Some(vr) = verify_rx.recv() => {
                process_verify_result(
                    vr, &info, &lengths, &mut piece_tracker,
                    &mut chunk_tracker, &mut peers, &storage, &stats,
                    &mut piece_assemblies,
                ).await;
                // New work may be available; re-drive all peers immediately
                // instead of waiting for the next 500 ms tick
                if !paused {
                    drive_requests(&mut peers, &mut piece_tracker, &mut chunk_tracker, max_outstanding).await;
                }
            }
            _ = tick.tick() => {
                let now = Instant::now();
                let dt = now.duration_since(last_tick).as_secs_f32().max(0.001);
                last_tick = now;
                // Reclaim chunk requests whose peer has been silent past
                // the request timeout. Without this, slow-but-TCP-alive
                // peers progressively hoard pieces (see REQUEST_TIMEOUT
                // docs); the symptom is downloads decaying over time
                // even while peer count stays constant.
                let reclaimed = chunk_tracker.reclaim_stale(REQUEST_TIMEOUT);
                if !reclaimed.is_empty() {
                    let mut unblocked_pieces: HashSet<u32> = HashSet::new();
                    for r in &reclaimed {
                        unblocked_pieces.insert(r.piece);
                        // Free the peer's outstanding slot so drive_peer
                        // can pipeline a different chunk. Without this,
                        // the slot stays consumed until the peer either
                        // delivers the (now reclaimed) chunk or trips
                        // the 120 s read timeout.
                        //
                        // Iterate every peer rather than only `r.peer`:
                        // in endgame mode the same chunk can be
                        // outstanding on multiple peers, but
                        // `ReclaimedChunk::peer` only carries the most
                        // recent `Requested { peer, .. }` writer.
                        // Skipping the others would permanently pin
                        // their request slots
                        for p in peers.values_mut() {
                            p.outstanding
                                .retain(|&(pi, be, _)| !(pi == r.piece && be == r.begin));
                        }
                    }
                    // A piece whose chunk got reclaimed may have been
                    // marked in-flight by drive_peer the last time it
                    // had no Missing chunks. Now that we made chunks
                    // Missing again, clear the in-flight flag so
                    // choose_requestable_piece returns it
                    for pi in unblocked_pieces {
                        if let Ok(vpi) = lengths.validate_piece(pi) {
                            piece_tracker.clear_in_flight(vpi);
                        }
                    }
                }
                let total_pieces = lengths.total_pieces() as usize;
                let peer_snaps: Vec<stats::PeerSnapshot> = peers
                    .values()
                    .map(|p| {
                        let seeder = peer_bitfield_is_full(&p.bitfield, total_pieces);
                        stats::PeerSnapshot {
                            addr: p.addr,
                            bitfield: p.bitfield.clone(),
                            am_choking: p.am_choking,
                            am_interested: p.am_interested,
                            peer_choking: p.peer_choking,
                            peer_interested: p.peer_interested,
                            seeder,
                        }
                    })
                    .collect();
                {
                    let mut s = stats.lock();
                    let upload_dt = upload_tick.swap(0, Ordering::Relaxed);
                    bytes_this_tick.1 = upload_dt;
                    s.live_stats.update(bytes_this_tick.0, bytes_this_tick.1, dt);
                    s.live_stats.snapshot.peer_stats.live = peers.len() as u32;
                    s.peers = peer_snaps;
                }
                bytes_this_tick = (0, 0);
                if !paused {
                    drive_requests(&mut peers, &mut piece_tracker, &mut chunk_tracker, max_outstanding).await;
                }
            }
        }
    }
}

/// Side-channel registry to pass outbound peer cmd_tx handles from the spawn
/// task into the main loop on first `Handshook`. Keyed by `(torrent_id, pid)`
/// because `pid` is only unique within a single torrent loop; a global
/// `pid` keying would collide across torrents.
mod peer_registry {
    use super::*;
    use once_cell::sync::Lazy;
    use std::sync::Mutex as StdMutex;
    static REG: Lazy<StdMutex<HashMap<(usize, u32), mpsc::Sender<PeerCommand>>>> =
        Lazy::new(|| StdMutex::new(HashMap::new()));
    pub fn put(torrent_id: usize, pid: u32, tx: mpsc::Sender<PeerCommand>) {
        REG.lock().unwrap().insert((torrent_id, pid), tx);
    }
    pub fn take(torrent_id: usize, pid: u32) -> Option<mpsc::Sender<PeerCommand>> {
        REG.lock().unwrap().remove(&(torrent_id, pid))
    }
}

fn spawn_outbound_peer(
    torrent_id: usize,
    pid: u32,
    addr: SocketAddr,
    info_hash: Id20,
    our_peer_id: Id20,
    event_tx: mpsc::Sender<(u32, PeerEvent)>,
    encryption: crate::peer::EncryptionPolicy,
) {
    tokio::spawn(async move {
        let spawn = SpawnPeer {
            addr,
            info_hash,
            our_peer_id,
            // 5 s is plenty for any reachable peer; 10 s used to park dial
            // slots for unreachable peers and starve real connections, since
            // a single tracker batch can include many dead addresses
            connect_timeout: Duration::from_secs(5),
            read_timeout: Duration::from_secs(120),
            encryption,
        };
        match connect(spawn).await {
            Ok((handle, mut rx)) => {
                peer_registry::put(torrent_id, pid, handle.tx.clone());
                while let Some(ev) = rx.recv().await {
                    if event_tx.send((pid, ev)).await.is_err() {
                        break;
                    }
                }
            }
            Err(e) => {
                let _ = event_tx
                    .send((
                        pid,
                        PeerEvent::Disconnected {
                            reason: format!("connect: {e}"),
                        },
                    ))
                    .await;
            }
        }
    });
}

async fn adopt_inbound_peer(
    pid: u32,
    addr: SocketAddr,
    cmd_tx: mpsc::Sender<PeerCommand>,
    mut event_rx: mpsc::Receiver<PeerEvent>,
    fwd_tx: mpsc::Sender<(u32, PeerEvent)>,
    peers: &mut HashMap<u32, Peer>,
    lengths: &Lengths,
    piece_tracker: &mut PieceTracker,
) {
    peers.insert(
        pid,
        Peer {
            addr,
            cmd_tx: cmd_tx.clone(),
            bitfield: vec![0u8; lengths.piece_bitfield_bytes()],
            am_choking: true,
            am_interested: false,
            peer_choking: true,
            peer_interested: false,
            outstanding: Vec::new(),
        },
    );
    // Seed bitfield + unchoke straight away
    let bf = piece_tracker.bitfield();
    let _ = cmd_tx
        .send(PeerCommand::Send(Message::Bitfield(Bytes::from(bf))))
        .await;
    let _ = cmd_tx.send(PeerCommand::Send(Message::Unchoke)).await;
    // Mark am_choking false so Request from peer is served
    if let Some(p) = peers.get_mut(&pid) {
        p.am_choking = false;
    }
    tokio::spawn(async move {
        while let Some(ev) = event_rx.recv().await {
            if fwd_tx.send((pid, ev)).await.is_err() {
                break;
            }
        }
    });
}

#[allow(clippy::too_many_arguments)]
async fn process_peer_event(
    torrent_id: usize,
    pid: u32,
    ev: PeerEvent,
    peers: &mut HashMap<u32, Peer>,
    piece_tracker: &mut PieceTracker,
    chunk_tracker: &mut ChunkTracker,
    piece_assemblies: &mut HashMap<u32, PieceAssembly>,
    lengths: &Lengths,
    storage: &Arc<FilesystemStorage>,
    stats: &Arc<Mutex<TorrentStats>>,
    bytes_this_tick: &mut (u64, u64),
    upload_tick: &Arc<AtomicU64>,
    write_tasks: &mut tokio::task::JoinSet<()>,
    pending_dials: &mut HashMap<u32, SocketAddr>,
    known_addrs: &mut HashSet<SocketAddr>,
    verify_tx: &mpsc::Sender<VerifyResult>,
    max_peers: usize,
) -> bool {
    // Return value: `true` if the caller should immediately kick the peer
    // request pipeline. Set for events that can free an outstanding slot
    // (Piece) or unblock requests (Unchoke, Bitfield, Have)
    let mut kick = false;
    match ev {
        PeerEvent::Handshook { .. } => {
            if !peers.contains_key(&pid) {
                if let Some(cmd_tx) = peer_registry::take(torrent_id, pid) {
                    // Move from pending dial to live peer if we tracked it.
                    let addr = pending_dials.remove(&pid).unwrap_or_else(|| {
                        // Fallback: unknown addr (shouldn't happen for outbound)
                        "0.0.0.0:0".parse().unwrap()
                    });
                    // Pending dials can outrun the max_peers cap (we allow
                    // up to MAX_PENDING_DIALS in flight). If we'd overflow,
                    // reject this freshly-handshook peer rather than
                    // exceeding the cap. Drop the address from
                    // known_addrs so it remains a candidate for future
                    // attempts when a slot frees
                    if peers.len() >= max_peers {
                        known_addrs.remove(&addr);
                        let _ = cmd_tx.send(PeerCommand::Disconnect).await;
                        return false;
                    }
                    peers.insert(
                        pid,
                        Peer {
                            addr,
                            cmd_tx: cmd_tx.clone(),
                            bitfield: vec![0u8; lengths.piece_bitfield_bytes()],
                            am_choking: true,
                            am_interested: false,
                            peer_choking: true,
                            peer_interested: false,
                            outstanding: Vec::new(),
                        },
                    );
                    let bf = piece_tracker.bitfield();
                    let _ = cmd_tx
                        .send(PeerCommand::Send(Message::Bitfield(Bytes::from(bf))))
                        .await;
                    let _ = cmd_tx.send(PeerCommand::Send(Message::Unchoke)).await;
                    if let Some(p) = peers.get_mut(&pid) {
                        p.am_choking = false;
                    }
                }
            }
        }
        PeerEvent::Message(msg) => {
            let Some(peer) = peers.get_mut(&pid) else {
                return false;
            };
            match msg {
                Message::Choke => peer.peer_choking = true,
                Message::Unchoke => {
                    peer.peer_choking = false;
                    kick = true;
                }
                Message::Interested => peer.peer_interested = true,
                Message::NotInterested => peer.peer_interested = false,
                Message::Have { piece_index } => {
                    let byte = (piece_index / 8) as usize;
                    let bit = 7 - (piece_index % 8) as u8;
                    if byte < peer.bitfield.len() {
                        peer.bitfield[byte] |= 1 << bit;
                    }
                    if let Ok(vpi) = lengths.validate_piece(piece_index) {
                        piece_tracker.note_peer_has(vpi);
                    }
                    send_interested_if_useful(peer, piece_tracker).await;
                    kick = true;
                }
                Message::Bitfield(bytes) => {
                    let n = bytes.len().min(peer.bitfield.len());
                    peer.bitfield[..n].copy_from_slice(&bytes[..n]);
                    piece_tracker.add_peer_bitfield(&peer.bitfield);
                    send_interested_if_useful(peer, piece_tracker).await;
                    kick = true;
                }
                Message::Request {
                    index,
                    begin,
                    length,
                } => {
                    let Ok(vpi) = lengths.validate_piece(index) else {
                        return false;
                    };
                    if !piece_tracker.has_local(vpi) || peer.am_choking {
                        return false;
                    }
                    if length > 1024 * 1024 {
                        return false;
                    }
                    // Reject requests that straddle the piece boundary so a
                    // malicious peer cannot trigger an out-of-bounds read.
                    let piece_len = lengths.piece_length_of(vpi) as u64;
                    if (begin as u64).saturating_add(length as u64) > piece_len {
                        return false;
                    }
                    let offset = lengths.piece_offset(vpi) + begin as u64;
                    // Offload the disk read + send to a task. Awaiting on the
                    // main loop here would stall every download peer for the
                    // duration of every upload `read_at`
                    let storage = storage.clone();
                    let cmd_tx = peer.cmd_tx.clone();
                    let stats = stats.clone();
                    let upload_tick = Arc::clone(upload_tick);
                    let upload_len = length as u64;
                    tokio::spawn(async move {
                        let mut buf = vec![0u8; length as usize];
                        if storage.read_at(offset, &mut buf).await.is_err() {
                            return;
                        }
                        if cmd_tx
                            .send(PeerCommand::Send(Message::Piece {
                                index,
                                begin,
                                data: Bytes::from(buf),
                            }))
                            .await
                            .is_err()
                        {
                            return;
                        }
                        // Only credit the upload after both the disk read
                        // and the send to the peer succeeded. Crediting
                        // before would over-report on read errors or when
                        // the peer's command channel was closed mid-flight
                        upload_tick.fetch_add(upload_len, Ordering::Relaxed);
                        stats.lock().uploaded_bytes += upload_len;
                    });
                }
                Message::Piece { index, begin, data } => {
                    let Ok(vpi) = lengths.validate_piece(index) else {
                        return false;
                    };
                    // Only accept pieces that match an outstanding request;
                    // reject unsolicited or mismatched frames.
                    let requested = peer.outstanding.iter().position(|&(p, b, l)| {
                        p == index && b == begin && l as usize == data.len()
                    });
                    let Some(req_idx) = requested else {
                        return false;
                    };
                    // Reject if data extends past the piece boundary.
                    let piece_len = lengths.piece_length_of(vpi) as u32;
                    if (begin as u64).saturating_add(data.len() as u64) > piece_len as u64 {
                        return false;
                    }
                    peer.outstanding.swap_remove(req_idx);
                    // Last use of `peer` — NLL releases the &mut peers borrow
                    // here so the cancel-scan below can re-borrow `peers`

                    // Drop late duplicates whose piece was already
                    // verified by another endgame peer. process_verify_result
                    // calls forget_piece, so accepting this would re-create
                    // a stale chunk-state vector via mark_received and
                    // reallocate a fresh PieceAssembly buffer — both would
                    // then leak until the torrent is dropped
                    if piece_tracker.has_local(vpi) {
                        kick = true;
                        return kick;
                    }

                    // Accumulate the chunk in an in-memory piece buffer
                    // instead of hitting disk per chunk. This is the key
                    // throughput unlock: the previous `storage.write_at(...).await`
                    // here serialised every download peer through one disk
                    // write per 16 KiB chunk
                    let assembly = piece_assemblies
                        .entry(index)
                        .or_insert_with(|| PieceAssembly {
                            buf: vec![0u8; piece_len as usize],
                            received_chunks: HashSet::new(),
                            received_bytes: 0,
                            expected_bytes: piece_len,
                            completed: false,
                        });
                    let chunk_index = begin / super::core::CHUNK_SIZE;
                    let begin_usz = begin as usize;
                    let end_usz = begin_usz + data.len();
                    let chunk_len = data.len();
                    // Drop late duplicates: in endgame mode the same chunk
                    // may arrive from multiple peers. Counting them again
                    // would inflate received_bytes past expected_bytes and
                    // skew progress reporting. Likewise ignore any chunk
                    // arriving after the assembly was handed off to the
                    // verify/write task
                    let accepted = !assembly.completed
                        && end_usz <= assembly.buf.len()
                        && assembly.received_chunks.insert(chunk_index);
                    if accepted {
                        assembly.buf[begin_usz..end_usz].copy_from_slice(&data);
                        assembly.received_bytes += chunk_len as u32;
                        // Only credit fresh bytes toward displayed download
                        // speed. Including duplicates would inflate the metric
                        // every time endgame's parallel requests collide
                        bytes_this_tick.0 += chunk_len as u64;
                    }
                    // Endgame parallel requests: now that we have this chunk
                    // (either fresh from this peer or already from someone
                    // else), tell every OTHER peer with the same outstanding
                    // request to stop sending it. Without this, redundant
                    // chunks keep saturating downstream and starve the
                    // remaining real requests near completion.
                    if chunk_tracker.endgame() {
                        cancel_outstanding(peers, pid, index, begin, chunk_len as u32);
                    }
                    let cinfo = super::core::ChunkInfo {
                        piece_index: vpi,
                        chunk_index: begin / super::core::CHUNK_SIZE,
                        absolute_index: 0,
                        size: data.len() as u32,
                        offset: begin,
                    };
                    let piece_done = chunk_tracker.mark_received(cinfo);
                    kick = true;
                    if piece_done {
                        // Piece complete: mark the assembly as completed and
                        // move its buffer out so duplicate chunks arriving
                        // after this point cannot mutate it. We keep the
                        // (now-empty) entry around as a sentinel so a stray
                        // late chunk does not recreate the assembly via
                        // entry().or_insert_with(...). The entry is removed
                        // by process_verify_result once the hash result
                        // lands
                        let Some(assembly) = piece_assemblies.get_mut(&index) else {
                            return kick;
                        };
                        if assembly.completed {
                            return kick;
                        }
                        if assembly.received_bytes != assembly.expected_bytes {
                            // Missing bytes from a peer that disconnected
                            // before all chunks arrived: hash will fail anyway,
                            // so just reset and re-request
                            piece_assemblies.remove(&index);
                            chunk_tracker.reset_piece(vpi);
                            return kick;
                        }
                        assembly.completed = true;
                        let buf = std::mem::take(&mut assembly.buf);
                        let poff = lengths.piece_offset(vpi);
                        let storage = storage.clone();
                        let verify_tx = verify_tx.clone();
                        write_tasks.spawn(async move {
                            // Wrap into Bytes for zero-copy share with both
                            // the SHA1 task and the disk write. `Bytes::from(Vec)`
                            // does not copy
                            let buf: bytes::Bytes = buf.into();
                            let hash_buf = buf.clone();
                            let hash_handle = tokio::task::spawn_blocking(move || {
                                let mut h = Sha1::new();
                                h.update(&hash_buf);
                                let out: [u8; 20] = h.finalize().into();
                                out
                            });
                            // If the write fails (ENOSPC, EIO, …) the data
                            // on disk is incomplete — signal that to the
                            // torrent loop so it does not mark the piece
                            // local. Sending a successful hash here would
                            // corrupt the resume state
                            let write_failed = storage.write_at_owned(poff, buf).await.is_err();
                            let hash = hash_handle.await.unwrap_or([0u8; 20]);
                            let _ = verify_tx
                                .send(VerifyResult {
                                    piece_index: index,
                                    hash,
                                    write_failed,
                                })
                                .await;
                        });
                    }
                }
                _ => {}
            }
        }
        PeerEvent::Disconnected { .. } => {
            // Clear from either in-flight or live, and release the address
            // for future retries (otherwise a single drop permanently
            // blacklists the peer).
            let addr = pending_dials
                .remove(&pid)
                .or_else(|| peers.get(&pid).map(|p| p.addr));
            if let Some(a) = addr {
                known_addrs.remove(&a);
            }
            // Drop the registry slot in case the peer disconnected before
            // the Handshook event moved it into `peers`.
            let _ = peer_registry::take(torrent_id, pid);
            if let Some(dead) = peers.remove(&pid) {
                piece_tracker.remove_peer_bitfield(&dead.bitfield);
                let freed = chunk_tracker.release_peer(pid);
                for piece_idx in freed {
                    if let Ok(vpi) = lengths.validate_piece(piece_idx) {
                        piece_tracker.clear_in_flight(vpi);
                    }
                    // Performant variant: keep the piece-assembly buffer when
                    // other peers have already contributed chunks. The freed
                    // `Requested` slots are now `Missing` (release_peer above)
                    // so other peers will refill them, and when the piece
                    // completes `assembly.received_bytes` will equal
                    // `expected_bytes`. Only drop the buffer if the dead peer
                    // was the sole contributor — keeping an empty assembly
                    // would just leak `piece_len` bytes per stalled piece.
                    //
                    // Invariant: every chunk index in `assembly.received_chunks`
                    // corresponds to ChunkState::Received in the chunk tracker,
                    // and release_peer only mutates Requested slots, so the
                    // tracker stays consistent with the buffer
                    let drop = piece_assemblies
                        .get(&piece_idx)
                        .is_none_or(|a| a.received_chunks.is_empty());
                    if drop {
                        piece_assemblies.remove(&piece_idx);
                    }
                }
            }
        }
    }
    kick
}

async fn process_verify_result(
    vr: VerifyResult,
    info: &ValidatedTorrentMetaV1Info,
    lengths: &Lengths,
    piece_tracker: &mut PieceTracker,
    chunk_tracker: &mut ChunkTracker,
    peers: &mut HashMap<u32, Peer>,
    storage: &Arc<FilesystemStorage>,
    stats: &Arc<Mutex<TorrentStats>>,
    piece_assemblies: &mut HashMap<u32, PieceAssembly>,
) {
    let Ok(vpi) = lengths.validate_piece(vr.piece_index) else {
        return;
    };
    // Always clear in-flight regardless of verification outcome
    piece_tracker.clear_in_flight(vpi);
    // Drop the (now-empty, completed=true) assembly sentinel so a future
    // failed verification can reallocate a fresh assembly on retry
    piece_assemblies.remove(&vr.piece_index);
    if vr.write_failed {
        log::warn!(
            "piece {} disk write failed; will re-request",
            vr.piece_index
        );
        // Stop any other endgame peer still streaming chunks of this piece;
        // they'd race the fresh re-request and force another reset
        cancel_piece_outstanding(peers, vr.piece_index);
        chunk_tracker.reset_piece(vpi);
        return;
    }
    match info.piece_hash(vr.piece_index) {
        Some(expected) if *expected.as_bytes() == vr.hash => {
            piece_tracker.set_local(vpi, true);
            // Piece is verified + on disk; its dense chunk state is
            // no longer needed. Dropping keeps `release_peer` and
            // `pending_chunks` bounded by the working set of
            // in-flight pieces rather than the torrent's lifetime
            // piece count
            chunk_tracker.forget_piece(vpi);
            // Tell every peer to stop sending the rest of this piece.
            // Without this, endgame duplicates of the remaining chunks
            // keep arriving (silently dropped by the has_local guard)
            // and saturate downstream during the final pieces
            cancel_piece_outstanding(peers, vr.piece_index);
            broadcast_have(peers, vr.piece_index).await;
            let mut s = stats.lock();
            s.progress_bytes = completed_bytes(piece_tracker, lengths);
            s.file_progress = compute_file_progress(piece_tracker, lengths, storage.layout());
            s.finished = piece_tracker.is_complete();
        }
        _ => {
            log::debug!("piece {} hash mismatch", vr.piece_index);
            // Same reasoning as write_failed: stale chunks from the prior
            // attempt would interleave with the re-request
            cancel_piece_outstanding(peers, vr.piece_index);
            chunk_tracker.reset_piece(vpi);
        }
    }
}

async fn send_interested_if_useful(peer: &mut Peer, piece_tracker: &mut PieceTracker) {
    if !peer.am_interested && piece_tracker.choose_piece(&peer.bitfield).is_some() {
        peer.am_interested = true;
        let _ = peer
            .cmd_tx
            .send(PeerCommand::Send(Message::Interested))
            .await;
    }
}

async fn broadcast_have(peers: &mut HashMap<u32, Peer>, piece_index: u32) {
    // Use try_send so a single backed-up peer can't stall the main loop;
    // if the channel is full the peer will get an updated bitfield via the
    // 500 ms tick anyway, and Have is best-effort
    for p in peers.values_mut() {
        let _ = p
            .cmd_tx
            .try_send(PeerCommand::Send(Message::Have { piece_index }));
    }
}

async fn drive_requests(
    peers: &mut HashMap<u32, Peer>,
    piece_tracker: &mut PieceTracker,
    chunk_tracker: &mut ChunkTracker,
    max_outstanding: usize,
) {
    let pids: Vec<u32> = peers.keys().copied().collect();
    for pid in pids {
        drive_peer(pid, peers, piece_tracker, chunk_tracker, max_outstanding).await;
    }
}

/// Pipeline requests for a single peer up to `max_outstanding`
/// Called both on the tick and inline after events that can unblock a peer
/// (Unchoke, Bitfield, Have, Piece). Without the inline calls, throughput
/// is capped at `max_outstanding * CHUNK_SIZE / tick_interval`
/// Send a `Cancel` message to every peer (other than `except_pid`) that has
/// the chunk `(index, begin, length)` in its outstanding queue, and remove
/// the entry from each peer's local outstanding Vec. Best-effort: if a
/// peer's command channel is full or closed we skip it (the chunk will
/// arrive and be dedup-dropped, no worse than today).
///
/// which is the only thing that keeps endgame mode from saturating downstream
/// with duplicate blocks at the very end of a download
fn cancel_outstanding(
    peers: &mut HashMap<u32, Peer>,
    except_pid: u32,
    index: u32,
    begin: u32,
    length: u32,
) {
    for (&other_pid, other) in peers.iter_mut() {
        if other_pid == except_pid {
            continue;
        }
        let Some(slot) = other
            .outstanding
            .iter()
            .position(|&(p, b, l)| p == index && b == begin && l == length)
        else {
            continue;
        };
        // Drop our local record first so a duplicate Piece response will be
        // rejected as unsolicited even if the peer ignores Cancel
        other.outstanding.swap_remove(slot);
        let _ = other.cmd_tx.try_send(PeerCommand::Send(Message::Cancel {
            index,
            begin,
            length,
        }));
    }
}

/// Send `Cancel` for every outstanding chunk request matching `piece_index`
/// across every peer. Called when the piece is no longer needed — either
/// because it was verified successfully (other peers' endgame duplicates
/// would now be wasted bytes) or because it failed verification / disk write
/// and is about to be re-requested from scratch (delivering stale chunks
/// from the previous attempt would inflate received_bytes past the
/// expected_bytes guard, forcing a second reset).
fn cancel_piece_outstanding(peers: &mut HashMap<u32, Peer>, piece_index: u32) {
    for other in peers.values_mut() {
        let mut i = 0;
        while i < other.outstanding.len() {
            let (p, begin, length) = other.outstanding[i];
            if p == piece_index {
                other.outstanding.swap_remove(i);
                let _ = other.cmd_tx.try_send(PeerCommand::Send(Message::Cancel {
                    index: piece_index,
                    begin,
                    length,
                }));
            } else {
                i += 1;
            }
        }
    }
}

async fn drive_peer(
    pid: u32,
    peers: &mut HashMap<u32, Peer>,
    piece_tracker: &mut PieceTracker,
    chunk_tracker: &mut ChunkTracker,
    max_outstanding: usize,
) {
    let Some(peer) = peers.get_mut(&pid) else {
        return;
    };
    if peer.peer_choking || !peer.am_interested {
        return;
    }
    // Pieces this peer already attempted in this drive_peer call but for which
    // next_chunk returned None (no chunk available for this peer, even under
    // endgame). Tracked so the endgame fallback below — which uses
    // PieceTracker::choose_piece (does NOT skip in_flight) — does not loop
    // forever picking the same piece every iteration.
    let mut exhausted: HashSet<u32> = HashSet::new();
    while peer.outstanding.len() < max_outstanding {
        // Use the peer id as a hint to distribute piece selection across
        // peers, avoiding the scenario where every peer picks the same piece.
        //
        // Endgame fallback: when no requestable (i.e. non-in-flight) piece
        // remains, flip endgame on if we're near completion and then ask the
        // tracker for ANY piece the peer has (including in-flight ones) that
        // we haven't already exhausted this call. ChunkTracker::next_chunk's
        // endgame branch will then duplicate another peer's outstanding chunk
        // request. Without this fallback, the endgame flag is set but never
        // observed, fast peers idle until REQUEST_TIMEOUT reclaims, and the
        // last 1% takes minutes.
        let piece = match piece_tracker.choose_requestable_piece(&peer.bitfield, pid) {
            Some(p) if !exhausted.contains(&p.get()) => p,
            _ => {
                if !chunk_tracker.endgame() && chunk_tracker.pending_chunks() <= 64 {
                    chunk_tracker.set_endgame(true);
                }
                if chunk_tracker.endgame() {
                    match piece_tracker.choose_piece_excluding(&peer.bitfield, pid, &exhausted) {
                        Some(p) => p,
                        None => break,
                    }
                } else {
                    break;
                }
            }
        };
        match chunk_tracker.next_chunk(piece, pid) {
            Some(chunk) => {
                let info = chunk.info;
                let prior = chunk.prior_state;
                // try_send + rollback so a single peer with a full writer
                // queue cannot block the entire main loop. send().await on
                // a full channel here would freeze every other download peer
                use tokio::sync::mpsc::error::TrySendError;
                let req = Message::Request {
                    index: info.piece_index.get(),
                    begin: info.offset,
                    length: info.size,
                };
                match peer.cmd_tx.try_send(PeerCommand::Send(req)) {
                    Ok(()) => {
                        peer.outstanding
                            .push((info.piece_index.get(), info.offset, info.size));
                    }
                    Err(TrySendError::Full(_)) => {
                        // Roll the chunk back so a different peer (or this
                        // one on the next tick) can pick it up
                        chunk_tracker.unrequest_chunk(info.piece_index, info.chunk_index, prior);
                        break;
                    }
                    Err(TrySendError::Closed(_)) => {
                        chunk_tracker.unrequest_chunk(info.piece_index, info.chunk_index, prior);
                        break;
                    }
                }
            }
            None => {
                // All chunks of this piece are already requested or received
                // (under endgame, also already requested by THIS peer). Mark
                // in_flight so non-endgame `choose_requestable_piece` skips
                // it, and record it locally so the endgame fallback path does
                // not pick it again on the next loop iteration
                piece_tracker.mark_in_flight(piece);
                exhausted.insert(piece.get());
                continue;
            }
        }
    }
}

async fn scan_existing_pieces(
    info: &ValidatedTorrentMetaV1Info,
    storage: &Arc<FilesystemStorage>,
    lengths: &Lengths,
    piece_tracker: &mut PieceTracker,
) {
    // Sequential scanning of every piece blocks the torrent loop before any
    // peer can connect. For a 50 GB torrent that is many seconds of dead
    // time. Hash pieces in parallel batches so disk reads + SHA1 overlap
    use tokio::task::JoinSet;
    let total = lengths.total_pieces();
    if total == 0 {
        return;
    }
    // Cap memory rather than parallelism: the previous fixed concurrency=16
    // allocated 16 * piece_length bytes per batch, which on torrents with
    // multi-MiB pieces could push hundreds of MiB through this scan.
    // Derive the batch size from a byte budget so each batch allocates at
    // most ~MAX_SCAN_BYTES, and still cap at 16 to avoid saturating the
    // spawn_blocking pool on tiny-piece torrents
    const MAX_SCAN_BYTES: usize = 64 * 1024 * 1024;
    let plen_hint = lengths
        .validate_piece(0)
        .map(|vpi| lengths.piece_length_of(vpi) as usize)
        .unwrap_or(0)
        .max(1);
    let concurrency = (MAX_SCAN_BYTES / plen_hint).clamp(1, 16);
    let mut idx: u32 = 0;
    while idx < total {
        let mut set: JoinSet<Option<(u32, [u8; 20])>> = JoinSet::new();
        let chunk_end = (idx + concurrency as u32).min(total);
        for i in idx..chunk_end {
            let Ok(vpi) = lengths.validate_piece(i) else {
                continue;
            };
            let plen = lengths.piece_length_of(vpi) as usize;
            let offset = lengths.piece_offset(vpi);
            let storage = storage.clone();
            set.spawn(async move {
                let mut buf = vec![0u8; plen];
                if storage.read_at(offset, &mut buf).await.is_err() {
                    return None;
                }
                let hash = tokio::task::spawn_blocking(move || {
                    let mut h = Sha1::new();
                    h.update(&buf);
                    let out: [u8; 20] = h.finalize().into();
                    out
                })
                .await
                .ok()?;
                Some((i, hash))
            });
        }
        while let Some(res) = set.join_next().await {
            if let Ok(Some((i, got))) = res {
                if let Some(expected) = info.piece_hash(i) {
                    if *expected.as_bytes() == got {
                        if let Ok(vpi) = lengths.validate_piece(i) {
                            piece_tracker.set_local(vpi, true);
                        }
                    }
                }
            }
        }
        idx = chunk_end;
    }
}

fn completed_bytes(pt: &PieceTracker, lengths: &Lengths) -> u64 {
    let mut total = 0u64;
    for idx in 0..lengths.total_pieces() {
        if let Ok(vpi) = lengths.validate_piece(idx) {
            if pt.has_local(vpi) {
                total += lengths.piece_length_of(vpi) as u64;
            }
        }
    }
    total
}

/// Distribute the bytes of every completed piece across the files it
/// overlaps. Used to populate `TorrentStats::file_progress` so per-file
/// completion can be reported in the UI.
fn compute_file_progress(
    pt: &PieceTracker,
    lengths: &Lengths,
    layout: &crate::storage::FileSet,
) -> Vec<u64> {
    let mut per_file = vec![0u64; layout.files().len()];
    for idx in 0..lengths.total_pieces() {
        let Ok(vpi) = lengths.validate_piece(idx) else {
            continue;
        };
        if !pt.has_local(vpi) {
            continue;
        }
        let offset = lengths.piece_offset(vpi);
        let len = lengths.piece_length_of(vpi) as u64;
        for span in layout.spans_for(offset, len) {
            per_file[span.file_index] += span.len;
        }
    }
    per_file
}

/// True when a peer's bitfield reports every piece, indicating a full seeder.
fn peer_bitfield_is_full(bitfield: &[u8], total_pieces: usize) -> bool {
    if total_pieces == 0 {
        return false;
    }
    let needed_bytes = total_pieces.div_ceil(8);
    if bitfield.len() < needed_bytes {
        return false;
    }
    let full_bytes = total_pieces / 8;
    if bitfield[..full_bytes].iter().any(|b| *b != 0xff) {
        return false;
    }
    let trailing_bits = total_pieces % 8;
    if trailing_bits == 0 {
        return true;
    }
    let mask: u8 = 0xffu8 << (8 - trailing_bits);
    bitfield[full_bytes] & mask == mask
}

fn collect_trackers(meta: &TorrentMeta) -> Vec<String> {
    let mut v = Vec::new();
    if let Some(a) = &meta.announce {
        v.push(a.clone());
    }
    for tier in &meta.announce_list {
        for url in tier {
            if !v.contains(url) {
                v.push(url.clone());
            }
        }
    }
    v
}

fn spawn_tracker_pollers(
    trackers: Vec<String>,
    info_hash: Id20,
    peer_id: Id20,
    port: u16,
    left: u64,
) -> mpsc::Receiver<SocketAddr> {
    let (tx, rx) = mpsc::channel(256);
    for url in trackers {
        let tx = tx.clone();
        tokio::spawn(async move {
            let mut event = AnnounceEvent::Started;
            loop {
                let req = AnnounceRequest {
                    info_hash,
                    peer_id,
                    port,
                    uploaded: 0,
                    downloaded: 0,
                    left,
                    event,
                    // Match aria2/libtorrent: ask for the maximum the BEP-3
                    // tracker will return. 50 used to leave us starved with
                    // single-tracker torrents
                    num_want: 200,
                };
                match tracker_announce(&url, &req, Duration::from_secs(30)).await {
                    Ok(resp) => {
                        for a in resp.peers {
                            if tx.send(a).await.is_err() {
                                return;
                            }
                        }
                        tokio::time::sleep(resp.interval).await;
                    }
                    Err(e) => {
                        log::debug!("tracker {url} failed: {e}");
                        tokio::time::sleep(Duration::from_secs(120)).await;
                    }
                }
                event = AnnounceEvent::None;
            }
        });
    }
    rx
}
