//! Per-torrent state machine

pub mod stats;

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use arc_swap::ArcSwapOption;
use bytes::Bytes;
use parking_lot::Mutex;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{interval, MissedTickBehavior};

use super::core::{supports_v2_wire, Id20, Lengths, MerkleProofTable, PieceVerifier, TorrentMeta};
use super::peer::{connect_with_utp_fallback, PeerCommand, PeerEvent, SpawnPeer};
use super::piece::{ChunkTracker, PieceTracker};
use super::storage::{FileSet, FilesystemStorage};
use super::tracker::{announce as tracker_announce, AnnounceEvent, AnnounceRequest};
use super::utp::UtpSocket;
use super::wire::extended::{
    build_holepunch, holepunch_err, holepunch_type, parse_holepunch, ut_metadata_data,
    ut_metadata_type, ExtHandshake, HolepunchMsg, EXT_HANDSHAKE_ID,
};
use super::wire::{Message, MessageEncoder};

pub use stats::{
    AggregatedLiveStats, LiveStats, PeerSnapshot, Snapshot, SpeedSample, TorrentStats,
};

const DEFAULT_MAX_OUTSTANDING_PER_PEER: usize = 6;
const UB_MAX_OUTSTANDING_PER_PEER: usize = 256;
const DEFAULT_MAX_PEERS: usize = 100;
const MAX_PENDING_DIALS: usize = 256;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);

const PEER_IDLE_TIMEOUT: Duration = Duration::from_secs(180);

const SNUB_EVICTION_TIMEOUT: Duration = Duration::from_secs(60);
const PEER_SNAPSHOT_INTERVAL: Duration = Duration::from_secs(5);

const UPLOAD_SLOTS: usize = 4;
const CHOKE_EVAL_INTERVAL: Duration = Duration::from_secs(10);
const OPTIMISTIC_ROTATE_INTERVAL: Duration = Duration::from_secs(30);
const PIPELINE_TARGET_SECS: f32 = 4.0;
const PIPELINE_RATE_WINDOW: Duration = Duration::from_secs(2);

const PEX_INTERVAL: Duration = Duration::from_secs(60);
const MAX_PEX_ADDED_PER_MSG: usize = 50;

const OUR_UT_METADATA_ID: u8 = 3;
const OUR_UT_PEX_ID: u8 = 4;
const OUR_UT_HOLEPUNCH_ID: u8 = 5;
const MAX_PEX_SOURCE_ENTRIES: usize = 4096;
const META_PIECE_SIZE: usize = 16 * 1024;

pub struct TorrentInit {
    pub meta: TorrentMeta,
    pub lengths: Lengths,
    pub root_dir: PathBuf,
    pub only_files: Option<Vec<usize>>,
    pub max_outstanding_per_peer: Option<usize>,
    pub max_peers: Option<usize>,
    pub encryption: super::peer::EncryptionPolicy,
    pub advertise_v2: bool,
    pub verifier: PieceVerifier,
    pub create_subfolder: bool,
    pub utp: Option<Arc<UtpSocket>>,
    pub upload_limiter: Option<Arc<crate::limiter::UploadLimiter>>,
    pub dht: Option<Arc<super::dht::Dht>>,
}

#[derive(Debug)]
pub enum TorrentCommand {
    AddPeer(SocketAddr),
    AddInboundPeer {
        addr: SocketAddr,
        cmd_tx: mpsc::Sender<PeerCommand>,
        event_rx: mpsc::Receiver<PeerEvent>,
        reserved: [u8; 8],
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
    pub create_subfolder: bool,
    pub root_dir: PathBuf,
    pub advertise_v2: Arc<AtomicBool>,
    pub ext_handshake_builder: crate::peer::ExtHandshakeBuilder,
    pub(crate) cmd_tx: mpsc::Sender<TorrentCommand>,
    pub(crate) stats: Arc<Mutex<TorrentStats>>,
}

impl ManagedTorrent {
    pub fn info_hash(&self) -> Id20 {
        self.info_hash
    }
    pub fn info_hash_v2(&self) -> Option<crate::core::hash::Id32> {
        self.metadata.load().as_ref().and_then(|m| m.info_hash_v2)
    }
    pub fn meta_version(&self) -> Option<&'static str> {
        self.metadata
            .load()
            .as_ref()
            .map(|m| m.meta_version.as_str())
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
    // `only_files` reaches TorrentInit, but the scheduler still fetches every piece
    // Warn so callers do not think a subset-only download is active
    if init.only_files.is_some() {
        tracing::warn!(
            "torrent {id}: selective download (only_files) is not yet supported; \
             downloading all files"
        );
    }
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
    let ext_handshake_builder: crate::peer::ExtHandshakeBuilder = {
        let metadata_size = init.meta.info_bytes.len() as u64;
        std::sync::Arc::new(move |peer_ip: std::net::IpAddr| {
            let hs =
                ExtHandshake::new_outgoing(OUR_UT_METADATA_ID, OUR_UT_PEX_ID, Some(metadata_size))
                    .with_holepunch(OUR_UT_HOLEPUNCH_ID)
                    .with_yourip(peer_ip);
            MessageEncoder::encode(&Message::Extended {
                ext_id: EXT_HANDSHAKE_ID,
                payload: hs.encode(),
            })
        })
    };
    let advertise_v2_flag = Arc::new(AtomicBool::new(init.advertise_v2));
    let handle = Arc::new(ManagedTorrent {
        id,
        info_hash,
        name,
        metadata: metadata_swap,
        create_subfolder: init.create_subfolder,
        root_dir: init.root_dir.clone(),
        advertise_v2: Arc::clone(&advertise_v2_flag),
        ext_handshake_builder,
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
        advertise_v2_flag,
        handle.ext_handshake_builder.clone(),
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
    max_outstanding: usize,
    delivered_window: u32,
    window_start: Instant,

    snubbing: bool,
    last_recv: Instant,
    snub_since: Option<Instant>,
    their_ut_metadata_id: Option<u8>,
    their_ut_holepunch_id: Option<u8>,
    downloaded_window: u64,
    uploaded_window: Arc<AtomicU64>,
    their_ut_pex_id: Option<u8>,
    pex_sent: HashSet<SocketAddr>,
    outbound: bool,
    supports_fast: bool,
}

struct VerifyResult {
    piece_index: u32,
    write_failed: bool,
    verify_ok: bool,
}

struct PieceAssembly {
    buf: Vec<u8>,
    received_chunks: HashSet<u32>,
    received_bytes: u32,
    expected_bytes: u32,
    /// Set once the piece has been handed off to the verify/write task.
    /// Late-arriving duplicates after this point must not recreate or
    /// mutate the assembly
    completed: bool,
}

#[allow(clippy::too_many_arguments)]
async fn torrent_loop(
    torrent_id: usize,
    init: TorrentInit,
    our_peer_id: Id20,
    listen_port: u16,
    mut cmd_rx: mpsc::Receiver<TorrentCommand>,
    stats: Arc<Mutex<TorrentStats>>,
    advertise_v2_flag: Arc<AtomicBool>,
    ext_handshake_builder: crate::peer::ExtHandshakeBuilder,
) {
    let info = Arc::new(init.meta.info.clone());
    let info_hash = init.meta.info_hash;
    // Raw bytes of the BEP-3 / BEP-52 `info` dict. Wrapped in `Arc` so
    // every per-peer ut_metadata response can borrow read-only without
    // cloning the (potentially large) byte buffer
    let info_bytes: Arc<Vec<u8>> = Arc::new(init.meta.info_bytes.clone());
    let lengths = init.lengths;
    let encryption = init.encryption;
    // Shared µTP endpoint (if any), handed to every outbound dial so a failed
    // TCP connect can retry over µTP
    let utp = init.utp.clone();
    let upload_limiter = init.upload_limiter.clone();
    let dht = init.dht.clone();
    // Preliminary: whether the meta supports v2 wire. Refined below to
    // `supports_v2_wire && hash_tables.is_some()` after table construction
    // so a failed build never leads us to announce truncated v2 hashes
    // or answer BEP-52 HASH_REQUEST messages without valid Merkle data
    let supports_v2 = supports_v2_wire(&init.meta);
    let verifier = init.verifier;
    // V2 Merkle tables for serving HASH_REQUEST — built from the meta for
    // any torrent that carries v2 data (pure-v2 or hybrid). Hybrid torrents
    // use V1Sha1 for piece verification but must still be able to serve
    // BEP-52 hash requests to v2 peers
    let hash_tables: Option<Arc<Vec<MerkleProofTable>>> = {
        if let PieceVerifier::V2Merkle { ref tables, .. } = verifier {
            Some(Arc::clone(tables))
        } else if supports_v2 {
            if let Some(ref v2) = init.meta.info_v2 {
                // Hybrid torrent: build Merkle tables from the meta's piece_layers
                let mut tbls = Vec::with_capacity(v2.files.len());
                let mut ok = true;
                for f in &v2.files {
                    let layer = init
                        .meta
                        .piece_layers
                        .get(&f.pieces_root)
                        .map(|v| v.as_slice())
                        .unwrap_or(&[]);
                    match super::core::MerkleProofTable::from_layer_bytes(
                        f.pieces_root,
                        f.length,
                        v2.piece_length,
                        layer,
                    ) {
                        Ok(t) => tbls.push(t),
                        Err(e) => {
                            tracing::warn!("hybrid torrent {info_hash}: could not build Merkle table for serving: {e}");
                            ok = false;
                            break;
                        }
                    }
                }
                if ok {
                    Some(Arc::new(tbls))
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        }
    };
    let serve_v2_layers = supports_v2 && hash_tables.is_some();
    let advertise_v2 = init.advertise_v2 && serve_v2_layers;
    advertise_v2_flag.store(advertise_v2, Ordering::Relaxed);
    let (pipeline_floor, pipeline_cap) = match init.max_outstanding_per_peer {
        Some(n) => {
            let n = n.max(1);
            (n, n)
        }
        None => (
            DEFAULT_MAX_OUTSTANDING_PER_PEER,
            UB_MAX_OUTSTANDING_PER_PEER,
        ),
    };
    let max_peers = init.max_peers.unwrap_or(DEFAULT_MAX_PEERS).max(1);
    tracing::info!(
        target: "diag",
        "torrent pipeline config: max_outstanding_per_peer={:?} -> floor={} cap={} max_peers={}",
        init.max_outstanding_per_peer, pipeline_floor, pipeline_cap, max_peers
    );
    let storage = Arc::new(FilesystemStorage::new(&info, &init.root_dir));
    if let Err(e) = storage.preallocate().await {
        tracing::warn!("preallocate failed for {info_hash}: {e}");
    }
    let mut piece_tracker = PieceTracker::new(lengths);
    let mut chunk_tracker = ChunkTracker::new(lengths);
    let mut piece_assemblies: HashMap<u32, PieceAssembly> = HashMap::new();
    scan_existing_pieces(&verifier, &storage, &lengths, &mut piece_tracker).await;
    {
        let mut s = stats.lock();
        s.file_progress = compute_file_progress(&piece_tracker, &lengths, storage.layout());
        s.progress_bytes = s.file_progress.iter().sum();
        s.finished = piece_tracker.is_complete();
    }

    // Truncated v2 hashes are only meaningful on trackers when we can
    // actually serve v2 piece layers; otherwise advertise the v1 info-hash
    // alone so peers we discover go through the v1 download path
    let announce_hashes = if serve_v2_layers {
        init.meta.announce_infohashes()
    } else {
        vec![info_hash]
    };
    // Unified peer-source channel: trackers and inbound PEX (ut_pex) both
    // feed discovered peer addresses here; the main loop drains it and dials
    // (with dedup + cap enforcement). DHT feeds peers via the AddPeer command
    let (peer_src_tx, mut peer_addr_rx) = mpsc::channel::<SocketAddr>(256);
    let mut tracker_tasks = spawn_tracker_pollers(
        peer_src_tx.clone(),
        collect_trackers(&init.meta),
        announce_hashes,
        our_peer_id,
        listen_port,
        Arc::clone(&stats),
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
    // BEP-55
    let mut pex_source: HashMap<SocketAddr, u32> = HashMap::new();
    let mut holepunch_attempted: HashSet<SocketAddr> = HashSet::new();
    // Outbound dials that have been spawned but whose peer has not completed
    // the BT handshake yet. Tracked separately so the max-peer cap accounts
    // for in-flight connection bursts, not just handshook peers
    let mut pending_dials: HashMap<u32, SocketAddr> = HashMap::new();
    let registry_scope = Arc::new(());
    let mut paused = false;
    let mut tick = interval(Duration::from_millis(500));
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut last_tick = Instant::now();
    let mut last_keepalive = Instant::now();
    let mut last_peer_snapshot = Instant::now() - PEER_SNAPSHOT_INTERVAL;
    let mut last_choke_eval = Instant::now();
    let mut last_window_reset = Instant::now();
    let mut last_optimistic_rotate = Instant::now();
    let mut optimistic_pid: Option<u32> = None;
    let mut choke_dirty = false;
    let mut last_pex = Instant::now();
    let mut bytes_this_tick = (0u64, 0u64);
    // Upload bytes accumulate from spawned send tasks; share via atomic so
    // we only credit them after the disk read and channel send succeed
    let upload_tick: Arc<AtomicU64> = Arc::new(AtomicU64::new(0));
    // Track in-flight piece write/hash tasks so Stop can wait for (or
    // cancel) them. Without this, a Stop racing with a piece-completion
    // write_at could leave a partially-written piece on disk while the
    // torrent loop has already returned
    let mut write_tasks: tokio::task::JoinSet<()> = tokio::task::JoinSet::new();
    let mut outbound_tasks: tokio::task::JoinSet<()> = tokio::task::JoinSet::new();

    loop {
        tokio::select! {
            Some(cmd) = cmd_rx.recv() => match cmd {
                TorrentCommand::AddPeer(addr) => {
                    if !paused
                        && peers.len() < max_peers
                        && pending_dials.len() < MAX_PENDING_DIALS
                        && known_addrs.insert(addr)
                    {
                        let pid = next_pid; next_pid += 1;
                        pending_dials.insert(pid, addr);
                        outbound_tasks.spawn(run_outbound_peer(
                            torrent_id,
                            pid,
                            addr,
                            registry_scope.clone(),
                            info_hash,
                            our_peer_id,
                            peer_event_tx.clone(),
                            encryption,
                            advertise_v2,
                            Some(ext_handshake_builder.clone()),
                            utp.clone(),
                        ));
                    }
                }
                TorrentCommand::AddInboundPeer { addr, cmd_tx, event_rx, reserved } => {
                    if !paused
                        && peers.len() < max_peers
                        && known_addrs.insert(addr)
                    {
                        let pid = next_pid; next_pid += 1;
                        adopt_inbound_peer(pid, addr, cmd_tx, event_rx, peer_event_tx.clone(), &mut peers, &lengths, &mut piece_tracker, pipeline_floor, reserved).await;
                    }
                }
                TorrentCommand::Pause(ack) => {
                    paused = true;
                    for (pid, p) in peers.drain() {
                        let _ = p.cmd_tx.send(PeerCommand::Disconnect).await;
                        release_peer_scheduler_state(
                            pid,
                            &p.bitfield,
                            &mut piece_tracker,
                            &mut chunk_tracker,
                            &mut piece_assemblies,
                            &lengths,
                        );
                    }
                    pending_dials.clear();
                    known_addrs.clear();
                    pex_source.clear();
                    holepunch_attempted.clear();
                    // Wait for in-flight piece write tasks before closing handles
                    while let Some(result) = write_tasks.join_next().await {
                        if let Err(e) = result {
                            tracing::warn!("write task failed during pause: {e}");
                        }
                    }
                    // Release the cached file descriptors
                    if let Err(e) = storage.close_handles().await {
                        tracing::warn!("failed to close storage handles on pause: {e}");
                    }
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
                    tracker_tasks.abort_all();
                    while let Some(result) = tracker_tasks.join_next().await {
                        if let Err(e) = result {
                            if !e.is_cancelled() {
                                tracing::warn!("tracker task failed during stop: {e}");
                            }
                        }
                    }
                    outbound_tasks.shutdown().await;
                    for pid in pending_dials.keys().copied().collect::<Vec<_>>() {
                        peer_registry::remove(torrent_id, pid, &registry_scope);
                    }
                    pending_dials.clear();
                    // Wait for in-flight write/verify tasks before Stop acknowledges disk completion
                    while let Some(result) = write_tasks.join_next().await {
                        if let Err(e) = result {
                            tracing::warn!("write task failed during stop: {e}");
                        }
                    }
                    // Flush and release cached file descriptors
                    if let Err(e) = storage.close_handles().await {
                        tracing::warn!("failed to close storage handles on stop: {e}");
                    }
                    let _ = ack.send(());
                    break;
                }
            },
            Some(addr) = peer_addr_rx.recv() => {
                if !paused
                    && peers.len() < max_peers
                    && pending_dials.len() < MAX_PENDING_DIALS
                    && known_addrs.insert(addr)
                {
                    let pid = next_pid; next_pid += 1;
                    pending_dials.insert(pid, addr);
                    outbound_tasks.spawn(run_outbound_peer(
                        torrent_id,
                        pid,
                        addr,
                        registry_scope.clone(),
                        info_hash,
                        our_peer_id,
                        peer_event_tx.clone(),
                        encryption,
                        advertise_v2,
                        Some(ext_handshake_builder.clone()),
                        utp.clone(),
                    ));
                }
            }
            Some((pid, ev)) = peer_event_rx.recv() => {
                let kick = process_peer_event(
                    torrent_id, pid, ev, &registry_scope, paused, &mut peers, &mut piece_tracker, &mut chunk_tracker,
                    &mut piece_assemblies,
                    &lengths, &storage, &stats, &mut bytes_this_tick,
                    &upload_tick,
                    &mut write_tasks,
                    &mut pending_dials, &mut known_addrs,
                    &mut pex_source, &mut holepunch_attempted,
                    &peer_src_tx,
                    &verify_tx,
                    &verifier,
                    &info_bytes,
                    hash_tables.as_deref().map(|v| &**v),
                    pipeline_floor,
                    pipeline_cap,
                    max_peers,
                    &mut choke_dirty,
                    &upload_limiter,
                    dht.as_ref(),
                ).await;
                if kick && !paused {
                    drive_peer(pid, &mut peers, &mut piece_tracker, &mut chunk_tracker).await;
                }
            }
            Some(vr) = verify_rx.recv() => {
                process_verify_result(
                    vr, &lengths, &mut piece_tracker,
                    &mut chunk_tracker, &mut peers, &storage, &stats,
                    &mut piece_assemblies,
                ).await;
                // New work may be available; re-drive all peers immediately
                // instead of waiting for the next 500 ms tick
                if !paused {
                    drive_requests(&mut peers, &mut piece_tracker, &mut chunk_tracker).await;
                }
            }
            result = outbound_tasks.join_next(), if !outbound_tasks.is_empty() => {
                if let Some(Err(e)) = result {
                    if !e.is_cancelled() {
                        tracing::warn!("outbound peer task failed: {e}");
                    }
                }
            }
            _ = tick.tick() => {
                let now = Instant::now();
                let dt = now.duration_since(last_tick).as_secs_f32().max(0.001);
                last_tick = now;
                if now.duration_since(last_keepalive) >= Duration::from_secs(90) {
                    last_keepalive = now;
                    for p in peers.values() {
                        let _ = p.cmd_tx.try_send(PeerCommand::Send(Message::KeepAlive));
                    }
                }
                // Reclaim chunk requests whose peer has been silent past
                // the request timeout. Without this, slow-but-TCP-alive
                // peers progressively hoard pieces (see REQUEST_TIMEOUT
                // docs); the symptom is downloads decaying over time
                // even while peer count stays constant
                let reclaimed = chunk_tracker.reclaim_stale(REQUEST_TIMEOUT);
                if !reclaimed.is_empty() {
                    let mut unblocked_pieces: HashSet<u32> = HashSet::new();
                    for r in &reclaimed {
                        if let Some(p) = peers.get_mut(&r.peer) {
                            if !p.snubbing {
                                p.snubbing = true;
                                p.snub_since = Some(Instant::now());
                                p.max_outstanding =
                                    shrink_pipeline(p.max_outstanding, pipeline_floor);
                                p.delivered_window = 0;
                                p.window_start = Instant::now();
                            }
                        }
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
                // Evict peers gone silent or snubbed too long. Without this
                // sweep their slot is held until the (currently absent)
                // reader-side timeout fires — i.e. never — so peers leak
                // permanently and `peers.len()` saturates at `max_peers`,
                // blocking fresh tracker / DHT addresses. This is the dominant
                // cause of "download speed is great at first then collapses to
                // 0 after a while": each silent / snubbed peer parks a slot,
                // and the reclaim/snub mechanism alone can't free it because
                // the peer never delivers anything to clear `snubbing`
                {
                    let mut to_evict: Vec<u32> = Vec::new();
                    for (&pid, p) in peers.iter() {
                        if now.duration_since(p.last_recv) > PEER_IDLE_TIMEOUT {
                            to_evict.push(pid);
                            continue;
                        }
                        if let Some(snub_at) = p.snub_since {
                            if now.duration_since(snub_at) > SNUB_EVICTION_TIMEOUT {
                                to_evict.push(pid);
                            }
                        }
                    }
                    for pid in to_evict {
                        if let Some(p) = peers.remove(&pid) {
                            // Free the peer's address so the tracker /
                            // DHT can re-dial it later. Drop bitfield
                            // contributions and chunk reservations the
                            // same way a Disconnected event would
                            known_addrs.remove(&p.addr);
                            let _ = p.cmd_tx.try_send(PeerCommand::Disconnect);
                            release_peer_scheduler_state(
                                pid,
                                &p.bitfield,
                                &mut piece_tracker,
                                &mut chunk_tracker,
                                &mut piece_assemblies,
                                &lengths,
                            );
                        }
                    }
                }
                if choke_dirty || now.duration_since(last_choke_eval) >= CHOKE_EVAL_INTERVAL {
                    let scheduled = now.duration_since(last_window_reset) >= CHOKE_EVAL_INTERVAL;
                    choke_dirty = false;
                    last_choke_eval = now;
                    if now.duration_since(last_optimistic_rotate) >= OPTIMISTIC_ROTATE_INTERVAL {
                        last_optimistic_rotate = now;
                        let mut interested: Vec<u32> = peers
                            .iter()
                            .filter(|(_, p)| p.peer_interested)
                            .map(|(&pid, _)| pid)
                            .collect();
                        interested.sort_unstable();
                        optimistic_pid = next_optimistic(&interested, optimistic_pid);
                    }
                    run_choke_eval(
                        &mut peers,
                        piece_tracker.is_complete(),
                        optimistic_pid,
                        scheduled,
                    );
                    if scheduled {
                        last_window_reset = now;
                    }
                }
                if now.duration_since(last_pex) >= PEX_INTERVAL {
                    last_pex = now;
                    let current: HashSet<SocketAddr> = peers
                        .values()
                        .filter(|p| p.outbound)
                        .map(|p| p.addr)
                        .collect();
                    for p in peers.values_mut() {
                        let Some(pex_id) = p.their_ut_pex_id else {
                            continue;
                        };
                        let added: Vec<SocketAddr> = current
                            .iter()
                            .filter(|a| **a != p.addr && !p.pex_sent.contains(a))
                            .take(MAX_PEX_ADDED_PER_MSG)
                            .copied()
                            .collect();
                        let dropped: Vec<SocketAddr> = p
                            .pex_sent
                            .iter()
                            .filter(|a| !current.contains(a))
                            .copied()
                            .collect();
                        if added.is_empty() && dropped.is_empty() {
                            continue;
                        }
                        for a in &dropped {
                            p.pex_sent.remove(a);
                        }
                        p.pex_sent.extend(added.iter().copied());
                        let payload = super::wire::extended::build_ut_pex(&added, &dropped);
                        let _ = p.cmd_tx.try_send(PeerCommand::Send(Message::Extended {
                            ext_id: pex_id,
                            payload,
                        }));
                    }
                }
                let peer_snaps = if now.duration_since(last_peer_snapshot) >= PEER_SNAPSHOT_INTERVAL {
                    last_peer_snapshot = now;
                    let total_pieces = lengths.total_pieces() as usize;
                    Some(
                        peers
                            .values()
                            .map(|p| {
                                let seeder = peer_bitfield_is_full(&p.bitfield, total_pieces);
                                stats::PeerSnapshot {
                                    addr: p.addr,
                                    bitfield: Arc::<[u8]>::from(p.bitfield.as_slice()),
                                    am_choking: p.am_choking,
                                    am_interested: p.am_interested,
                                    peer_choking: p.peer_choking,
                                    peer_interested: p.peer_interested,
                                    seeder,
                                }
                            })
                            .collect(),
                    )
                } else {
                    None
                };
                {
                    let mut s = stats.lock();
                    let upload_dt = upload_tick.swap(0, Ordering::Relaxed);
                    bytes_this_tick.1 = upload_dt;
                    s.live_stats.update(bytes_this_tick.0, bytes_this_tick.1, dt);
                    s.live_stats.snapshot.peer_stats.live = peers.len() as u32;
                    if let Some(peer_snaps) = peer_snaps {
                        s.peers = peer_snaps;
                    }
                }
                tracing::debug!(
                    target: "diag",
                    "TICK summary peers={} pending_dials={} known={} endgame={} dl_bytes_tick={} ul_bytes_tick={} pending_chunks={} dt_ms={:.0}",
                    peers.len(),
                    pending_dials.len(),
                    known_addrs.len(),
                    chunk_tracker.endgame(),
                    bytes_this_tick.0,
                    bytes_this_tick.1,
                    chunk_tracker.pending_chunks(),
                    f64::from(dt) * 1000.0
                );
                bytes_this_tick = (0, 0);
                if !paused {
                    drive_requests(&mut peers, &mut piece_tracker, &mut chunk_tracker).await;
                }
            }
        }
    }
}

/// Side-channel registry to pass outbound peer cmd_tx handles from the spawn
/// task into the main loop on first `Handshook`. Keyed by `(torrent_id, pid)`
/// because `pid` is only unique within a single torrent loop; a global
/// `pid` keying would collide across torrents
mod peer_registry {
    use super::*;
    use std::sync::LazyLock;
    use std::sync::Mutex as StdMutex;

    struct RegistryEntry {
        scope: Arc<()>,
        tx: mpsc::Sender<PeerCommand>,
        addr: SocketAddr,
    }

    type PeerCmdRegistry = StdMutex<HashMap<(usize, u32), RegistryEntry>>;
    static REG: LazyLock<PeerCmdRegistry> = LazyLock::new(|| StdMutex::new(HashMap::new()));

    pub fn put(
        torrent_id: usize,
        pid: u32,
        scope: &Arc<()>,
        tx: mpsc::Sender<PeerCommand>,
        addr: SocketAddr,
    ) {
        REG.lock().unwrap().insert(
            (torrent_id, pid),
            RegistryEntry {
                scope: scope.clone(),
                tx,
                addr,
            },
        );
    }

    pub fn take(
        torrent_id: usize,
        pid: u32,
        scope: &Arc<()>,
    ) -> Option<(mpsc::Sender<PeerCommand>, SocketAddr)> {
        let mut reg = REG.lock().unwrap();
        let key = (torrent_id, pid);
        if !reg
            .get(&key)
            .is_some_and(|entry| Arc::ptr_eq(&entry.scope, scope))
        {
            return None;
        }
        reg.remove(&key).map(|entry| (entry.tx, entry.addr))
    }

    pub fn remove(torrent_id: usize, pid: u32, scope: &Arc<()>) {
        let mut reg = REG.lock().unwrap();
        let key = (torrent_id, pid);
        if reg
            .get(&key)
            .is_some_and(|entry| Arc::ptr_eq(&entry.scope, scope))
        {
            reg.remove(&key);
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_outbound_peer(
    torrent_id: usize,
    pid: u32,
    addr: SocketAddr,
    registry_scope: Arc<()>,
    info_hash: Id20,
    our_peer_id: Id20,
    event_tx: mpsc::Sender<(u32, PeerEvent)>,
    encryption: crate::peer::EncryptionPolicy,
    advertise_v2: bool,
    ext_handshake_builder: Option<crate::peer::ExtHandshakeBuilder>,
    utp: Option<Arc<UtpSocket>>,
) {
    let spawn = SpawnPeer {
        addr,
        info_hash,
        our_peer_id,
        connect_timeout: Duration::from_secs(10),
        read_timeout: Duration::from_secs(120),
        encryption,
        advertise_v2,
        ext_handshake_builder,
    };
    match connect_with_utp_fallback(spawn, utp).await {
        Ok((handle, mut rx)) => {
            peer_registry::put(
                torrent_id,
                pid,
                &registry_scope,
                handle.tx.clone(),
                handle.addr,
            );
            while let Some(ev) = rx.recv().await {
                if event_tx.send((pid, ev)).await.is_err() {
                    break;
                }
            }
            peer_registry::remove(torrent_id, pid, &registry_scope);
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
}

fn fast_bit(reserved: &[u8; 8]) -> bool {
    let (b, m) = super::wire::handshake::reserved::FAST;
    reserved[b] & m != 0
}

#[allow(clippy::too_many_arguments)]
async fn adopt_inbound_peer(
    pid: u32,
    addr: SocketAddr,
    cmd_tx: mpsc::Sender<PeerCommand>,
    mut event_rx: mpsc::Receiver<PeerEvent>,
    fwd_tx: mpsc::Sender<(u32, PeerEvent)>,
    peers: &mut HashMap<u32, Peer>,
    lengths: &Lengths,
    piece_tracker: &mut PieceTracker,
    pipeline_floor: usize,
    reserved: [u8; 8],
) {
    let supports_fast = fast_bit(&reserved);
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
            max_outstanding: pipeline_floor,
            delivered_window: 0,
            window_start: Instant::now(),
            snubbing: false,
            last_recv: Instant::now(),
            snub_since: None,
            their_ut_metadata_id: None,
            their_ut_holepunch_id: None,
            downloaded_window: 0,
            uploaded_window: Arc::new(AtomicU64::new(0)),
            their_ut_pex_id: None,
            pex_sent: HashSet::new(),
            outbound: false,
            supports_fast,
        },
    );
    let bf = piece_tracker.bitfield();
    if should_send_initial_bitfield(&bf) {
        let _ = cmd_tx
            .send(PeerCommand::Send(Message::Bitfield(Bytes::from(bf))))
            .await;
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
    registry_scope: &Arc<()>,
    paused: bool,
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
    // BEP-55: addr gossiped via PEX -> the relay pid that gossiped it
    pex_source: &mut HashMap<SocketAddr, u32>,
    // BEP-55: targets we've already asked a relay to rendezvous
    holepunch_attempted: &mut HashSet<SocketAddr>,
    peer_src_tx: &mpsc::Sender<SocketAddr>,
    verify_tx: &mpsc::Sender<VerifyResult>,
    verifier: &PieceVerifier,
    info_bytes: &Arc<Vec<u8>>,
    hash_tables: Option<&[MerkleProofTable]>,
    pipeline_floor: usize,
    pipeline_cap: usize,
    max_peers: usize,
    choke_dirty: &mut bool,
    upload_limiter: &Option<Arc<crate::limiter::UploadLimiter>>,
    dht: Option<&Arc<crate::dht::Dht>>,
) -> bool {
    // Return value: `true` if the caller should immediately kick the peer
    // request pipeline. Set for events that can free an outstanding slot
    // (Piece) or unblock requests (Unchoke, Bitfield, Have)
    let mut kick = false;
    match ev {
        PeerEvent::Handshook {
            encrypted,
            reserved,
            ..
        } => {
            if !peers.contains_key(&pid) {
                if let Some((cmd_tx, registry_addr)) =
                    peer_registry::take(torrent_id, pid, registry_scope)
                {
                    // The torrent paused while this dial was in flight
                    // The take() removed the registry entry; drop `cmd_tx` and disconnect
                    if paused {
                        pending_dials.remove(&pid);
                        let _ = cmd_tx.send(PeerCommand::Disconnect).await;
                        return false;
                    }
                    // Move from pending dial to live peer. The registry is
                    // the authoritative source of `addr` because
                    // `pending_dials` may have been cleared by Pause/Stop
                    // while the connect+handshake was in flight; the
                    // registry entry is only ever written by the spawn
                    // task that actually completed the TCP connect
                    let addr = pending_dials.remove(&pid).unwrap_or(registry_addr);
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
                    tracing::debug!(
                        "peer connected: {addr} (encrypted={encrypted}, peers={}/{max_peers})",
                        peers.len() + 1
                    );
                    let supports_fast = fast_bit(&reserved);
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
                            max_outstanding: pipeline_floor,
                            delivered_window: 0,
                            window_start: Instant::now(),
                            snubbing: false,
                            last_recv: Instant::now(),
                            snub_since: None,
                            their_ut_metadata_id: None,
                            their_ut_holepunch_id: None,
                            downloaded_window: 0,
                            uploaded_window: Arc::new(AtomicU64::new(0)),
                            their_ut_pex_id: None,
                            pex_sent: HashSet::new(),
                            outbound: true,
                            supports_fast,
                        },
                    );
                    // Extended handshake (when peer supports BEP-10) is
                    // already on the wire — the connection layer wrote it
                    // synchronously before the Handshook event was emitted
                    let bf = piece_tracker.bitfield();
                    if should_send_initial_bitfield(&bf) {
                        let _ = cmd_tx
                            .send(PeerCommand::Send(Message::Bitfield(Bytes::from(bf))))
                            .await;
                    }
                    // Choked until the tit-for-tat eval grants a slot
                }
            }
        }
        PeerEvent::Message(msg) => {
            let Some(peer) = peers.get_mut(&pid) else {
                return false;
            };
            // Refresh per-peer liveness on every wire message (including
            // KeepAlive / Choke / Have, not just Piece). The torrent loop's
            // tick uses this to evict peers idle past PEER_IDLE_TIMEOUT
            // and recycle their slot to a fresh dial \u2014 see the eviction
            // sweep in the `tick.tick()` arm
            peer.last_recv = Instant::now();
            if tracing::enabled!(target: "diag", tracing::Level::DEBUG) {
                let kind = match &msg {
                    Message::Have { piece_index } => format!("Have({piece_index})"),
                    Message::Bitfield(b) => {
                        let set: u32 = b.iter().map(|x| x.count_ones()).sum();
                        format!("Bitfield(len={} set_bits={})", b.len(), set)
                    }
                    Message::Piece { index, begin, data } => {
                        format!("Piece(i={index} b={begin} len={})", data.len())
                    }
                    Message::Unknown { id, payload } => {
                        format!("Unknown(id={id} len={})", payload.len())
                    }
                    other => format!("{other:?}"),
                };
                tracing::debug!(
                    target: "diag",
                    "RX {} {kind} am_interested={} peer_choking={} am_choking={}",
                    peer.addr, peer.am_interested, peer.peer_choking, peer.am_choking
                );
            }
            match msg {
                Message::Choke => peer.peer_choking = true,
                Message::Unchoke => {
                    peer.peer_choking = false;
                    kick = true;
                }
                Message::Interested => {
                    peer.peer_interested = true;
                    *choke_dirty = true;
                }
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
                Message::HaveAll => {
                    piece_tracker.remove_peer_bitfield(&peer.bitfield);
                    for b in peer.bitfield.iter_mut() {
                        *b = 0xff;
                    }
                    piece_tracker.add_peer_bitfield(&peer.bitfield);
                    send_interested_if_useful(peer, piece_tracker).await;
                    kick = true;
                }
                Message::HaveNone => {
                    piece_tracker.remove_peer_bitfield(&peer.bitfield);
                    for b in peer.bitfield.iter_mut() {
                        *b = 0;
                    }
                }
                Message::RejectRequest {
                    index,
                    begin,
                    length,
                } => {
                    peer.outstanding
                        .retain(|&(p, b, l)| !(p == index && b == begin && l == length));
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
                        if peer.supports_fast {
                            let _ =
                                peer.cmd_tx
                                    .try_send(PeerCommand::Send(Message::RejectRequest {
                                        index,
                                        begin,
                                        length,
                                    }));
                        }
                        return false;
                    }
                    if length > 1024 * 1024 {
                        return false;
                    }
                    // Reject requests that straddle the piece boundary so a
                    // malicious peer cannot trigger an out-of-bounds read
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
                    let peer_uploaded = Arc::clone(&peer.uploaded_window);
                    let upload_len = length as u64;
                    let limiter = upload_limiter.clone();
                    tokio::spawn(async move {
                        if let Some(l) = &limiter {
                            l.acquire(upload_len).await;
                        }
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
                        peer_uploaded.fetch_add(upload_len, Ordering::Relaxed);
                        stats.lock().uploaded_bytes += upload_len;
                    });
                }
                Message::Piece { index, begin, data } => {
                    let Ok(vpi) = lengths.validate_piece(index) else {
                        return false;
                    };
                    // Only accept pieces that match an outstanding request;
                    // reject unsolicited or mismatched frames
                    let requested = peer.outstanding.iter().position(|&(p, b, l)| {
                        p == index && b == begin && l as usize == data.len()
                    });
                    let Some(req_idx) = requested else {
                        return false;
                    };
                    // Reject if data extends past the piece boundary
                    let piece_len = lengths.piece_length_of(vpi);
                    if (begin as u64).saturating_add(data.len() as u64) > piece_len as u64 {
                        return false;
                    }
                    peer.outstanding.swap_remove(req_idx);
                    // Clear the snubbing flag the moment any
                    // chunk arrives: this is what makes snubbing a soft,
                    // self-healing back-off rather than a permanent
                    // demotion. A peer that timed out earlier in this
                    // session is allowed back into the request rotation
                    // as soon as it proves it can still deliver bytes
                    peer.snubbing = false;
                    peer.snub_since = None;
                    peer.downloaded_window += data.len() as u64;
                    peer.delivered_window += 1;
                    let window = peer.window_start.elapsed();
                    if window >= PIPELINE_RATE_WINDOW {
                        let target = pipeline_target(
                            peer.delivered_window,
                            window,
                            pipeline_floor,
                            pipeline_cap,
                        );
                        if target != peer.max_outstanding {
                            tracing::debug!(
                                target: "diag",
                                "pipeline TARGET pid={pid} addr={} {}->{target}",
                                peer.addr, peer.max_outstanding
                            );
                            peer.max_outstanding = target;
                        }
                        peer.delivered_window = 0;
                        peer.window_start = Instant::now();
                    }
                    if piece_tracker.has_local(vpi) {
                        kick = true;
                        return kick;
                    }

                    // Accumulate the chunk in an in-memory piece buffer
                    // instead of hitting disk per chunk. The previous
                    // `storage.write_at(...).await` here serialised every
                    // download peer through one disk write per 16 KiB chunk
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
                    // Guard with `accepted`: a duplicate Piece arrival in
                    // endgame means we've already accepted this chunk before
                    // (and already sent Cancel for it). Re-scanning every
                    // peer's outstanding Vec for late duplicates is pure
                    // O(n_peers) waste
                    if accepted && chunk_tracker.endgame() {
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
                            maybe_clear_endgame(chunk_tracker);
                            return kick;
                        }
                        assembly.completed = true;
                        let buf = std::mem::take(&mut assembly.buf);
                        let poff = lengths.piece_offset(vpi);
                        let storage = storage.clone();
                        let verify_tx = verify_tx.clone();
                        let verifier = verifier.clone();
                        write_tasks.spawn(async move {
                            // Wrap into Bytes for zero-copy share with both
                            // the verifier and the disk write. `Bytes::from(Vec)`
                            // does not copy
                            let buf: bytes::Bytes = buf.into();
                            let bytes_for_verify = buf.clone();
                            // Verify on the blocking pool in parallel with
                            // the disk write. Doing this here (not on the
                            // torrent loop) keeps the loop's select arm
                            // free to service peer events while N concurrent
                            // pieces hash in parallel
                            let verify_handle = tokio::task::spawn_blocking(move || {
                                verifier.verify(index, &bytes_for_verify).is_ok()
                            });
                            // If the write fails (ENOSPC, EIO, …) the data
                            // on disk is incomplete — signal that to the
                            // torrent loop so it does not mark the piece local
                            let write_failed = storage.write_at_owned(poff, buf).await.is_err();
                            let verify_ok = verify_handle.await.unwrap_or(false);
                            let _ = verify_tx
                                .send(VerifyResult {
                                    piece_index: index,
                                    write_failed,
                                    verify_ok,
                                })
                                .await;
                        });
                    }
                }
                // BEP 52 hash-exchange messages. We honour requests for an entire
                // piece-layer at the piece base layer (the common shape used by the
                // magnet resolver) by serving from the v2 verifier's `MerkleProofTable`.
                // Other request shapes (sub-piece-layer leaf requests, partial ranges
                // with proof_layers > 0) are answered with `HashReject`—a valid BEP 52
                // outcome that prompts the peer to fall back to v1 or another seeder.
                // Inbound `Hashes` / `HashReject` we did not request are dropped
                Message::HashRequest {
                    pieces_root,
                    base_layer,
                    index,
                    length,
                    proof_layers,
                } => {
                    let response = build_hash_response(
                        hash_tables,
                        pieces_root,
                        base_layer,
                        index,
                        length,
                        proof_layers,
                    );
                    // try_send: a `.await` here would stall the entire torrent loop on a
                    // single peer's backed-up writer queue. HashReject / Hashes are
                    // best-effort—if the peer's command channel is full it will time out
                    // its own request and either retry or fall back to v1
                    let _ = peer.cmd_tx.try_send(PeerCommand::Send(response));
                }
                Message::Hashes { .. } | Message::HashReject { .. } => {
                    // Discard: no outstanding HASH_REQUEST to correlate
                }
                // BEP-10 extended messages. We handle:
                //  - The handshake itself (`ext_id == 0`): record the peer's `ut_metadata`
                //    id so subsequent REQUESTs can be validated
                //  - `ut_metadata` REQUESTs (`ext_id == OUR_UT_METADATA_ID`): serve a 16 KiB
                //    block of our raw info dict, or REJECT for out-of-range pieces
                //  - `ut_pex` (`ext_id == OUR_UT_PEX_ID`): BEP-11 peer exchange—feed gossiped
                //    peers into the dial path
                Message::Extended { ext_id, payload } => {
                    if ext_id == EXT_HANDSHAKE_ID {
                        if let Some(peer_ext) = ExtHandshake::decode(&payload) {
                            peer.their_ut_metadata_id = peer_ext.ut_metadata_id();
                            peer.their_ut_holepunch_id = peer_ext.ut_holepunch_id();
                            peer.their_ut_pex_id = peer_ext.ut_pex_id();
                        }
                    } else if ext_id == OUR_UT_METADATA_ID {
                        serve_ut_metadata(peer, &payload, info_bytes);
                    } else if ext_id == OUR_UT_PEX_ID {
                        // A connected peer (often a seeder) gossips other swarm members
                        if let Some((v4, v6)) = super::wire::extended::parse_ut_pex(&payload) {
                            for addr in v4.into_iter().chain(v6) {
                                if pex_source.len() < MAX_PEX_SOURCE_ENTRIES
                                    || pex_source.contains_key(&addr)
                                {
                                    pex_source.insert(addr, pid);
                                }
                                let _ = peer_src_tx.try_send(addr);
                            }
                        }
                    } else if ext_id == OUR_UT_HOLEPUNCH_ID {
                        // BEP-55 hole punching
                        let from_addr = peer.addr;
                        let from_hp_id = peer.their_ut_holepunch_id;
                        let from_cmd = peer.cmd_tx.clone();
                        if let Some(hp) = parse_holepunch(&payload) {
                            handle_holepunch(
                                hp,
                                from_addr,
                                from_hp_id,
                                &from_cmd,
                                peers,
                                known_addrs,
                                peer_src_tx,
                            );
                        }
                    }
                }
                Message::Port(port) => {
                    if let (Some(dht), true) = (dht, port != 0) {
                        dht.add_node(SocketAddr::new(peer.addr.ip(), port));
                    }
                }
                _ => {}
            }
        }
        PeerEvent::Disconnected { reason } => {
            // Clear from either in-flight or live, and release the address for future
            // retries (otherwise a single drop permanently blacklists the peer)
            let pending_addr = pending_dials.remove(&pid);
            let was_pending_dial = pending_addr.is_some();
            let addr = pending_addr.or_else(|| peers.get(&pid).map(|p| p.addr));
            if let Some(a) = addr {
                // Log the disconnect cause at debug so the per-peer error —
                // typically the MSE handshake outcome for unreachable /
                // encryption-only peers — is visible to operators
                // troubleshooting "0 KB/s" reports without us having to
                // re-deduce it from "trying mse" lines alone
                tracing::debug!("peer {a} disconnected: {reason}");
                known_addrs.remove(&a);
                pex_source.retain(|_, relay| *relay != pid);
                if was_pending_dial {
                    try_initiate_holepunch(a, pex_source, holepunch_attempted, peers);
                }
            }
            // Drop the registry slot in case the peer disconnected before
            // the Handshook event moved it into `peers`
            peer_registry::remove(torrent_id, pid, registry_scope);
            if let Some(dead) = peers.remove(&pid) {
                *choke_dirty = true;
                release_peer_scheduler_state(
                    pid,
                    &dead.bitfield,
                    piece_tracker,
                    chunk_tracker,
                    piece_assemblies,
                    lengths,
                );
            }
        }
    }
    kick
}

#[allow(clippy::too_many_arguments)]
async fn process_verify_result(
    vr: VerifyResult,
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
        tracing::warn!(
            "piece {} disk write failed; will re-request",
            vr.piece_index
        );
        // Stop any other endgame peer still streaming chunks of this piece;
        // they'd race the fresh re-request and force another reset
        cancel_piece_outstanding(peers, vr.piece_index);
        chunk_tracker.reset_piece(vpi);
        maybe_clear_endgame(chunk_tracker);
        return;
    }
    // Verification has already completed in the per-piece write task
    // (see write_tasks.spawn in process_peer_event). Doing the hash here
    // would re-serialise every piece completion through the torrent
    // loop's select arm and stall all peer events for the hash duration
    if vr.verify_ok {
        let became_local = mark_verified_piece_local(piece_tracker, vpi);
        // Piece is verified + on disk; its dense chunk state is no longer
        // needed. Dropping keeps `release_peer` and `pending_chunks`
        // bounded by the working set of in-flight pieces rather than the
        // torrent's lifetime piece count
        chunk_tracker.forget_piece(vpi);
        // Tell every peer to stop sending the rest of this piece. Without
        // this, endgame duplicates of the remaining chunks keep arriving
        // (silently dropped by the has_local guard) and saturate
        // downstream during the final pieces
        cancel_piece_outstanding(peers, vr.piece_index);
        broadcast_have(peers, vr.piece_index).await;
        let mut s = stats.lock();
        if became_local {
            add_piece_progress(&mut s, lengths, storage.layout(), vpi);
        }
        s.finished = piece_tracker.is_complete();
    } else {
        tracing::debug!("piece {} verify failed", vr.piece_index);
        // Same reasoning as write_failed: stale chunks from the prior
        // attempt would interleave with the re-request
        cancel_piece_outstanding(peers, vr.piece_index);
        chunk_tracker.reset_piece(vpi);
        maybe_clear_endgame(chunk_tracker);
    }
}

/// Clear the endgame flag once the working set has grown back above the activation
/// threshold. Endgame is otherwise a one-way ratchet (set when `pending_chunks() <= 64`,
/// never cleared), which prevents pieces re-queued via `reset_piece` after a
/// hash/write failure from regaining the cheap sequential-scan path in `next_chunk`
/// and forces every peer through the `choose_piece_excluding` fallback for the rest
/// of the download
fn maybe_clear_endgame(chunk_tracker: &mut ChunkTracker) {
    if chunk_tracker.endgame() && chunk_tracker.pending_chunks() > 64 {
        chunk_tracker.set_endgame(false);
    }
}

/// Handle an inbound BEP-55 ut_holepunch message
fn handle_holepunch(
    hp: HolepunchMsg,
    from_addr: SocketAddr,
    from_hp_id: Option<u8>,
    from_cmd: &mpsc::Sender<PeerCommand>,
    peers: &HashMap<u32, Peer>,
    known_addrs: &mut HashSet<SocketAddr>,
    peer_src_tx: &mpsc::Sender<SocketAddr>,
) {
    match hp.msg_type {
        holepunch_type::CONNECT => {
            // A relay told us to connect to `hp.addr` now
            known_addrs.remove(&hp.addr);
            let _ = peer_src_tx.try_send(hp.addr);
        }
        holepunch_type::RENDEZVOUS => {
            // We are the relay
            enum Relay {
                Connect(mpsc::Sender<PeerCommand>, u8),
                Err(u32),
            }
            let action = if hp.addr == from_addr {
                Relay::Err(holepunch_err::NO_SELF)
            } else {
                match peers.values().find(|p| p.addr == hp.addr) {
                    Some(t) => match t.their_ut_holepunch_id {
                        Some(thp) => Relay::Connect(t.cmd_tx.clone(), thp),
                        None => Relay::Err(holepunch_err::NO_SUPPORT),
                    },
                    None => Relay::Err(holepunch_err::NO_SUCH_PEER),
                }
            };
            match action {
                Relay::Connect(target_cmd, target_hp) => {
                    // Tell the target to connect to the initiator
                    let _ = target_cmd.try_send(PeerCommand::Send(Message::Extended {
                        ext_id: target_hp,
                        payload: build_holepunch(holepunch_type::CONNECT, from_addr, 0),
                    }));
                    // and tell the initiator to connect to the target
                    if let Some(fid) = from_hp_id {
                        let _ = from_cmd.try_send(PeerCommand::Send(Message::Extended {
                            ext_id: fid,
                            payload: build_holepunch(holepunch_type::CONNECT, hp.addr, 0),
                        }));
                    }
                }
                Relay::Err(code) => {
                    if let Some(fid) = from_hp_id {
                        let _ = from_cmd.try_send(PeerCommand::Send(Message::Extended {
                            ext_id: fid,
                            payload: build_holepunch(holepunch_type::ERROR, hp.addr, code),
                        }));
                    }
                }
            }
        }
        holepunch_type::ERROR => {
            tracing::debug!(
                target: "diag",
                "holepunch ERROR from {from_addr} target={} code={}",
                hp.addr, hp.err_code
            );
        }
        _ => {}
    }
}

/// On a failed direct dial to `target`, ask the peer that gossiped it via
/// PEX to perform a BEP-55 rendezvous
fn try_initiate_holepunch(
    target: SocketAddr,
    pex_source: &HashMap<SocketAddr, u32>,
    holepunch_attempted: &mut HashSet<SocketAddr>,
    peers: &HashMap<u32, Peer>,
) {
    if holepunch_attempted.contains(&target) {
        return;
    }
    // Only peers we learned via PEX have a known relay; tracker/DHT peers that
    // fail to connect have no rendezvous path we can use
    let Some(&relay_pid) = pex_source.get(&target) else {
        return;
    };
    let Some(relay) = peers.get(&relay_pid) else {
        return; // the gossiper has since disconnected
    };
    let Some(relay_hp) = relay.their_ut_holepunch_id else {
        return; // relay doesn't support holepunch
    };
    if relay
        .cmd_tx
        .try_send(PeerCommand::Send(Message::Extended {
            ext_id: relay_hp,
            payload: build_holepunch(holepunch_type::RENDEZVOUS, target, 0),
        }))
        .is_ok()
    {
        holepunch_attempted.insert(target);
        tracing::debug!(
            target: "diag",
            "holepunch RENDEZVOUS initiate target={target} via relay pid={relay_pid}"
        );
    }
}

async fn send_interested_if_useful(peer: &mut Peer, piece_tracker: &mut PieceTracker) {
    let useful = piece_tracker.choose_piece(&peer.bitfield).is_some();
    let set_bits: u32 = peer.bitfield.iter().map(|x| x.count_ones()).sum();
    tracing::debug!(
        target: "diag",
        "send_interested_if_useful {} am_interested={} useful={} my_bitfield_set={}",
        peer.addr, peer.am_interested, useful, set_bits
    );
    if !peer.am_interested && useful {
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
) {
    let pids: Vec<u32> = peers.keys().copied().collect();
    for pid in pids {
        drive_peer(pid, peers, piece_tracker, chunk_tracker).await;
    }
}

fn pipeline_target(delivered_chunks: u32, window: Duration, floor: usize, cap: usize) -> usize {
    let secs = window.as_secs_f32().max(0.001);
    let rate = delivered_chunks as f32 / secs; // chunks per second
    ((rate * PIPELINE_TARGET_SECS) as usize).clamp(floor, cap)
}

fn shrink_pipeline(current: usize, floor: usize) -> usize {
    (current / 2).max(floor)
}

fn next_optimistic(sorted_candidates: &[u32], current: Option<u32>) -> Option<u32> {
    if sorted_candidates.is_empty() {
        return None;
    }
    let next = match current {
        Some(cur) => sorted_candidates
            .iter()
            .copied()
            .find(|&pid| pid > cur)
            .unwrap_or(sorted_candidates[0]),
        None => sorted_candidates[0],
    };
    Some(next)
}

fn select_unchoked(
    mut candidates: Vec<(u32, u64)>,
    optimistic: Option<u32>,
    slots: usize,
) -> HashSet<u32> {
    let mut selected: HashSet<u32> = HashSet::new();
    if let Some(pid) = optimistic.filter(|pid| candidates.iter().any(|&(c, _)| c == *pid)) {
        selected.insert(pid);
    }
    // Rate desc, pid asc as a deterministic tiebreak
    candidates.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    for (pid, _) in candidates {
        if selected.len() >= slots {
            break;
        }
        selected.insert(pid);
    }
    selected
}

fn run_choke_eval(
    peers: &mut HashMap<u32, Peer>,
    seeding: bool,
    optimistic: Option<u32>,
    reset_windows: bool,
) {
    let candidates: Vec<(u32, u64)> = peers
        .iter()
        .filter(|(_, p)| p.peer_interested)
        .map(|(&pid, p)| {
            let rate = if seeding {
                p.uploaded_window.load(Ordering::Relaxed)
            } else {
                p.downloaded_window
            };
            (pid, rate)
        })
        .collect();
    let unchoke = select_unchoked(candidates, optimistic, UPLOAD_SLOTS);
    for (&pid, p) in peers.iter_mut() {
        let should_unchoke = unchoke.contains(&pid);
        if should_unchoke
            && p.am_choking
            && p.cmd_tx
                .try_send(PeerCommand::Send(Message::Unchoke))
                .is_ok()
        {
            p.am_choking = false;
        } else if !should_unchoke
            && !p.am_choking
            && p.cmd_tx.try_send(PeerCommand::Send(Message::Choke)).is_ok()
        {
            p.am_choking = true;
        }
        if reset_windows {
            p.downloaded_window = 0;
            p.uploaded_window.store(0, Ordering::Relaxed);
        }
    }
}

/// Send a `Cancel` message to every peer (other than `except_pid`) that has
/// the chunk `(index, begin, length)` in its outstanding queue, and remove
/// the entry from each peer's local outstanding Vec. Best-effort: if a
/// peer's command channel is full or closed we skip it (the chunk will
/// arrive and be dedup-dropped, no worse than today). This is the only
/// thing that keeps endgame mode from saturating downstream with duplicate
/// blocks at the very end of a download
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
/// expected_bytes guard, forcing a second reset)
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

/// Pipeline requests for a single peer up to its adaptive
/// `max_outstanding`. Called both on the tick and inline after events
/// that can unblock a peer (Unchoke, Bitfield, Have, Piece). Without the
/// inline calls, throughput is capped at
/// `max_outstanding * CHUNK_SIZE / tick_interval`
async fn drive_peer(
    pid: u32,
    peers: &mut HashMap<u32, Peer>,
    piece_tracker: &mut PieceTracker,
    chunk_tracker: &mut ChunkTracker,
) {
    let Some(peer) = peers.get_mut(&pid) else {
        return;
    };
    if peer.peer_choking || !peer.am_interested {
        return;
    }
    // Snubbing: skip peers whose previous request timed out.
    // The flag clears on the next `Piece` arrival from this peer, so this
    // is a transient park, not a permanent ban. Without it, drive_peer
    // would immediately refill the slot we just freed in the reclaim
    // path, retriggering the timeout each REQUEST_TIMEOUT cycle and
    // wasting global request budget on a stuck peer
    if peer.snubbing {
        return;
    }
    let max_outstanding = peer.max_outstanding;
    // Pieces this peer already attempted in this drive_peer call but for which
    // next_chunk returned None (no chunk available for this peer, even under
    // endgame). Tracked so the endgame fallback below — which uses
    // PieceTracker::choose_piece (does NOT skip in_flight) — does not loop
    // forever picking the same piece every iteration
    let mut exhausted: HashSet<u32> = HashSet::new();
    let requestable_pieces = piece_tracker.choose_requestable_pieces(&peer.bitfield, pid);
    let mut requestable_idx = 0usize;
    let mut endgame_pieces: Option<Vec<super::core::ValidPieceIndex>> = None;
    let mut endgame_idx = 0usize;
    let mut current_piece: Option<super::core::ValidPieceIndex> = None;
    while peer.outstanding.len() < max_outstanding {
        let piece = match current_piece {
            Some(piece) => piece,
            None => {
                let mut selected = None;
                loop {
                    if let Some(piece) = requestable_pieces.get(requestable_idx).copied() {
                        requestable_idx += 1;
                        if !exhausted.contains(&piece.get()) {
                            selected = Some(piece);
                            break;
                        }
                        continue;
                    }

                    if !chunk_tracker.endgame() && chunk_tracker.pending_chunks() <= 64 {
                        chunk_tracker.set_endgame(true);
                    }
                    if chunk_tracker.endgame() {
                        let pieces = endgame_pieces.get_or_insert_with(|| {
                            piece_tracker.choose_pieces(&peer.bitfield, pid)
                        });
                        if let Some(piece) = pieces.get(endgame_idx).copied() {
                            endgame_idx += 1;
                            if !exhausted.contains(&piece.get()) {
                                selected = Some(piece);
                                break;
                            }
                            continue;
                        }
                    }
                    break;
                }
                let Some(piece) = selected else {
                    break;
                };
                piece
            }
        };
        match chunk_tracker.next_chunk(piece, pid) {
            Some(chunk) => {
                current_piece = Some(piece);
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
                current_piece = None;
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
    verifier: &PieceVerifier,
    storage: &Arc<FilesystemStorage>,
    lengths: &Lengths,
    piece_tracker: &mut PieceTracker,
) {
    // Sequential scanning of every piece blocks the torrent loop before any
    // peer can connect. For a 50 GB torrent that is many seconds of dead
    // time. Hash pieces in parallel batches so disk reads + verify overlap
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
        let mut set: JoinSet<Option<u32>> = JoinSet::new();
        let chunk_end = (idx + concurrency as u32).min(total);
        for i in idx..chunk_end {
            let Ok(vpi) = lengths.validate_piece(i) else {
                continue;
            };
            let plen = lengths.piece_length_of(vpi) as usize;
            let offset = lengths.piece_offset(vpi);
            let storage = storage.clone();
            let verifier = verifier.clone();
            set.spawn(async move {
                let mut buf = vec![0u8; plen];
                if storage.read_at(offset, &mut buf).await.is_err() {
                    return None;
                }
                let ok = tokio::task::spawn_blocking(move || verifier.verify(i, &buf).is_ok())
                    .await
                    .ok()?;
                if ok {
                    Some(i)
                } else {
                    None
                }
            });
        }
        while let Some(res) = set.join_next().await {
            if let Ok(Some(i)) = res {
                if let Ok(vpi) = lengths.validate_piece(i) {
                    piece_tracker.set_local(vpi, true);
                }
            }
        }
        idx = chunk_end;
    }
}

fn mark_verified_piece_local(pt: &mut PieceTracker, vpi: super::core::ValidPieceIndex) -> bool {
    let was_local = pt.has_local(vpi);
    pt.set_local(vpi, true);
    !was_local
}

fn add_piece_progress(
    stats: &mut TorrentStats,
    lengths: &Lengths,
    layout: &FileSet,
    vpi: super::core::ValidPieceIndex,
) {
    let offset = lengths.piece_offset(vpi);
    let len = lengths.piece_length_of(vpi) as u64;
    stats.progress_bytes = stats.progress_bytes.saturating_add(len);
    if stats.file_progress.len() < layout.files().len() {
        stats.file_progress.resize(layout.files().len(), 0);
    }
    for span in layout.spans_for(offset, len) {
        stats.file_progress[span.file_index] =
            stats.file_progress[span.file_index].saturating_add(span.len);
    }
}

/// Distribute the bytes of every completed piece across the files it
/// overlaps. Used to populate `TorrentStats::file_progress` so per-file
/// completion can be reported in the UI
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

/// True when a peer's bitfield reports every piece — a full seeder
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

fn should_send_initial_bitfield(bitfield: &[u8]) -> bool {
    bitfield.iter().any(|b| *b != 0)
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
    tx: mpsc::Sender<SocketAddr>,
    trackers: Vec<String>,
    info_hashes: Vec<Id20>,
    peer_id: Id20,
    port: u16,
    stats: Arc<Mutex<TorrentStats>>,
) -> tokio::task::JoinSet<()> {
    let mut tasks = tokio::task::JoinSet::new();
    for url in trackers {
        for info_hash in &info_hashes {
            let tx = tx.clone();
            let url = url.clone();
            let info_hash = *info_hash;
            let stats = Arc::clone(&stats);
            tasks.spawn(async move {
                let mut event = AnnounceEvent::Started;
                // Track whether we have already announced `Completed` to
                // this tracker so we only emit it once per session, even if
                // the torrent finishes mid-poll loop. Without this,
                // re-announcing `Completed` every interval would inflate
                // tracker-side completion counters
                let mut sent_completed = false;
                loop {
                    let (uploaded, downloaded, left, finished) = {
                        let s = stats.lock();
                        let left = s.total_bytes.saturating_sub(s.progress_bytes);
                        (s.uploaded_bytes, s.progress_bytes, left, s.finished)
                    };
                    // Promote to `Completed` the first time we observe
                    // `finished` after Started; trackers use this signal
                    // to move us into the seeders bucket and start
                    // returning leechers (who will actually request from
                    // us) instead of fellow seeders (who won't)
                    if finished && !sent_completed && !matches!(event, AnnounceEvent::Started) {
                        event = AnnounceEvent::Completed;
                    }
                    let req = AnnounceRequest {
                        info_hash,
                        peer_id,
                        port,
                        uploaded,
                        downloaded,
                        left,
                        event,
                        // Match aria2/libtorrent: ask for the maximum the BEP-3
                        // tracker will return. 50 used to leave us starved with
                        // single-tracker torrents
                        num_want: 200,
                    };
                    match tracker_announce(&url, &req, Duration::from_secs(30)).await {
                        Ok(resp) => {
                            // DIAG: per-URL peer yield — quantifies whether Risuko's
                            // (default-empty) tracker set is starving the swarm vs BitComet's
                            tracing::info!(
                                target: "diag",
                                "tracker ANNOUNCE ok url={url} event={event:?} peers={} interval_s={}",
                                resp.peers.len(),
                                resp.interval.as_secs()
                            );
                            if matches!(event, AnnounceEvent::Completed) {
                                sent_completed = true;
                            }
                            for a in resp.peers {
                                if tx.send(a).await.is_err() {
                                    return;
                                }
                            }
                            tokio::time::sleep(resp.interval).await;
                        }
                        Err(e) => {
                            tracing::info!(target: "diag", "tracker ANNOUNCE fail url={url} event={event:?} err={e}");
                            tokio::time::sleep(Duration::from_secs(120)).await;
                        }
                    }
                    event = AnnounceEvent::None;
                }
            });
        }
    }
    tasks
}

fn release_peer_scheduler_state(
    pid: u32,
    bitfield: &[u8],
    piece_tracker: &mut PieceTracker,
    chunk_tracker: &mut ChunkTracker,
    piece_assemblies: &mut HashMap<u32, PieceAssembly>,
    lengths: &Lengths,
) {
    piece_tracker.remove_peer_bitfield(bitfield);
    let freed = chunk_tracker.release_peer(pid);
    for piece_idx in freed {
        if let Ok(vpi) = lengths.validate_piece(piece_idx) {
            piece_tracker.clear_in_flight(vpi);
        }
        let should_drop = piece_assemblies
            .get(&piece_idx)
            .is_none_or(|a| a.received_chunks.is_empty());
        if should_drop {
            piece_assemblies.remove(&piece_idx);
        }
    }
}

/// Build a `HASHES` / `HashReject` reply for an inbound BEP 52
/// `HASH_REQUEST`. We honour requests that ask for an entire piece-layer
/// row (`base_layer == log2(piece_length / 16 KiB)`, `index == 0`,
/// `length == next_pow2(piece_count)`, `proof_layers == 0`), which is the
/// shape the magnet resolver uses. All other request shapes are answered
/// with `HashReject` — the peer can then fall back to v1 verification or
/// another seeder. This is a valid BEP 52 outcome
///
/// `tables` is the optional slice of per-file Merkle proof tables. It is
/// `Some` for pure-v2 and hybrid torrents and `None` for pure-v1 torrents.
/// Passing the tables separately (rather than the verifier) means hybrid
/// torrents — which use `V1Sha1` for piece verification — can still serve
/// BEP-52 HASH_REQUEST to v2 peers
fn build_hash_response(
    tables: Option<&[MerkleProofTable]>,
    pieces_root: [u8; 32],
    base_layer: u32,
    index: u32,
    length: u32,
    proof_layers: u32,
) -> Message {
    let reject = || Message::HashReject {
        pieces_root,
        base_layer,
        index,
        length,
        proof_layers,
    };

    let Some(tables) = tables else {
        return reject();
    };

    // Only support the "entire piece layer at once, no proof" shape
    if index != 0 || proof_layers != 0 {
        return reject();
    }

    let root = super::core::Id32(pieces_root);
    let Some(table) = tables.iter().find(|t| t.file_root == root) else {
        return reject();
    };
    if base_layer != table.piece_layer_base() || length != table.piece_layer_padded_len() {
        return reject();
    }
    let Some(payload) = table.serve_full_piece_layer() else {
        return reject();
    };
    Message::Hashes {
        pieces_root,
        base_layer,
        index,
        length,
        proof_layers,
        hashes: bytes::Bytes::from(payload),
    }
}

/// Serve a single inbound BEP-9 `ut_metadata` request. The peer's payload
/// is a bencoded `{msg_type, piece}` dict (DATA / REQUEST / REJECT). We
/// only act on REQUEST; for in-range pieces we reply with DATA carrying
/// the corresponding 16 KiB block of `info_bytes`, otherwise REJECT
fn serve_ut_metadata(peer: &Peer, payload: &Bytes, info_bytes: &Arc<Vec<u8>>) {
    let Some(msg) = super::wire::extended::parse_ut_metadata(payload.clone()) else {
        return;
    };
    if msg.msg_type != ut_metadata_type::REQUEST {
        // We never initiate a request from the torrent loop, so DATA /
        // REJECT replies here are unsolicited \u2014 ignore
        return;
    }
    let total = info_bytes.len();
    let total_pieces = total.div_ceil(META_PIECE_SIZE);
    let piece = msg.piece;
    if piece < 0 || (piece as usize) >= total_pieces {
        // Only send REJECT when the peer has told us which ext_id to use;
        // if we haven't received their handshake yet, sending on an id they
        // don't recognise is useless and potentially confusing
        if let Some(their_id) = peer.their_ut_metadata_id {
            let reject = super::wire::extended::ut_metadata_reject(piece);
            let _ = peer.cmd_tx.try_send(PeerCommand::Send(Message::Extended {
                ext_id: their_id,
                payload: reject,
            }));
        }
        return;
    }
    let Some(their_id) = peer.their_ut_metadata_id else {
        // Peer asked for ut_metadata without first telling us which ext
        // id to use. Drop silently \u2014 a well-behaved client always
        // sends its handshake first
        return;
    };
    let start = piece as usize * META_PIECE_SIZE;
    let end = (start + META_PIECE_SIZE).min(total);
    let block = &info_bytes[start..end];
    let data = ut_metadata_data(piece, total as i64, block);
    // try_send: never block the torrent loop on a slow peer's writer queue
    let _ = peer.cmd_tx.try_send(PeerCommand::Send(Message::Extended {
        ext_id: their_id,
        payload: data,
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::merkle::{compute_root, hash_block, MerkleProofTable, BLOCK_SIZE};
    use crate::core::Id32;
    use crate::core::TorrentMetaInfo;
    use crate::core::ValidatedTorrentMetaV1Info;
    use std::path::Path;

    #[test]
    fn pipeline_target_tracks_delivery_rate() {
        assert_eq!(pipeline_target(128, Duration::from_secs(2), 6, 256), 256);
        assert_eq!(pipeline_target(2, Duration::from_secs(2), 6, 256), 6);
        assert_eq!(pipeline_target(20, Duration::from_secs(2), 6, 256), 40);
        assert_eq!(pipeline_target(128, Duration::from_secs(2), 32, 32), 32);
    }

    #[test]
    fn pipeline_depth_shrinks_after_stall() {
        // The reclaim path applies exactly this on each snub transition
        assert_eq!(shrink_pipeline(256, 6), 128);
        assert_eq!(shrink_pipeline(128, 6), 64);
        assert_eq!(shrink_pipeline(8, 6), 6);
        assert_eq!(shrink_pipeline(6, 6), 6);
    }

    fn make_v2_tables(num_pieces: usize, piece_length: u32) -> (Vec<MerkleProofTable>, Id32) {
        let blocks_per_piece = piece_length / BLOCK_SIZE;
        let total = num_pieces as u64 * piece_length as u64;
        let mut data = vec![0u8; total as usize];
        for (i, b) in data.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(31).wrapping_add(7);
        }
        // Per-piece root: SHA-256 Merkle subtree over 16 KiB blocks,
        // zero-padded to `blocks_per_piece`
        let piece_roots: Vec<Id32> = data
            .chunks(piece_length as usize)
            .map(|p| {
                let mut leaves: Vec<Id32> = p.chunks(BLOCK_SIZE as usize).map(hash_block).collect();
                let target = blocks_per_piece as usize;
                if leaves.len() < target {
                    leaves.resize(target, Id32([0u8; 32]));
                }
                if target == 1 {
                    leaves[0]
                } else {
                    compute_root(&leaves)
                }
            })
            .collect();
        let file_root = if piece_roots.len() == 1 {
            piece_roots[0]
        } else {
            compute_root(&piece_roots)
        };
        let mut layer_bytes = Vec::with_capacity(piece_roots.len() * 32);
        for r in &piece_roots {
            layer_bytes.extend_from_slice(&r.0);
        }
        let table =
            MerkleProofTable::from_layer_bytes(file_root, total, piece_length, &layer_bytes)
                .expect("build table");
        (vec![table], file_root)
    }

    fn two_file_layout() -> (Lengths, FileSet) {
        let info = ValidatedTorrentMetaV1Info {
            name: "root".into(),
            piece_length: 10,
            pieces: vec![0; 20 * 3],
            private: false,
            files: vec![
                TorrentMetaInfo {
                    path: vec!["a".into()],
                    length: 15,
                },
                TorrentMetaInfo {
                    path: vec!["b".into()],
                    length: 15,
                },
            ],
            single_file_mode: false,
        };
        let lengths = Lengths::new(30, 10).unwrap();
        let layout = FileSet::from_meta(&info, Path::new("/tmp"));
        (lengths, layout)
    }

    #[test]
    fn verified_piece_progress_is_idempotent() {
        let (lengths, layout) = two_file_layout();
        let mut stats = TorrentStats::initial(
            lengths.total_length(),
            layout.files().iter().map(|f| f.length).collect(),
        );
        let mut tracker = PieceTracker::new(lengths);
        let piece = tracker.lengths().validate_piece(1).unwrap();

        if mark_verified_piece_local(&mut tracker, piece) {
            add_piece_progress(&mut stats, tracker.lengths(), &layout, piece);
        }
        assert_eq!(stats.progress_bytes, 10);
        assert_eq!(stats.file_progress, vec![5, 5]);

        if mark_verified_piece_local(&mut tracker, piece) {
            add_piece_progress(&mut stats, tracker.lengths(), &layout, piece);
        }
        assert_eq!(stats.progress_bytes, 10);
        assert_eq!(stats.file_progress, vec![5, 5]);
    }

    #[test]
    fn peer_registry_scope_prevents_cross_session_take() {
        let torrent_id = 7;
        let pid = 3;
        let addr: SocketAddr = "127.0.0.1:6881".parse().unwrap();
        let scope_a = Arc::new(());
        let scope_b = Arc::new(());
        let (tx, _rx) = mpsc::channel(1);

        peer_registry::put(torrent_id, pid, &scope_a, tx, addr);

        assert!(peer_registry::take(torrent_id, pid, &scope_b).is_none());
        assert!(peer_registry::take(torrent_id, pid, &scope_a).is_some());
    }

    #[test]
    fn build_hash_response_serves_full_piece_layer() {
        let (tables, root) = make_v2_tables(4, 64 * 1024);
        let base = tables[0].piece_layer_base();
        let length = tables[0].piece_layer_padded_len();
        let expected = tables[0].serve_full_piece_layer().unwrap();

        match build_hash_response(Some(&tables), root.0, base, 0, length, 0) {
            Message::Hashes {
                pieces_root,
                base_layer,
                index,
                length: l,
                proof_layers,
                hashes,
            } => {
                assert_eq!(pieces_root, root.0);
                assert_eq!(base_layer, base);
                assert_eq!(index, 0);
                assert_eq!(l, length);
                assert_eq!(proof_layers, 0);
                assert_eq!(hashes.as_ref(), expected.as_slice());
            }
            other => panic!("expected Hashes, got {other:?}"),
        }
    }

    #[test]
    fn initial_bitfield_is_suppressed_when_empty() {
        assert!(!should_send_initial_bitfield(&[]));
        assert!(!should_send_initial_bitfield(&[0]));
        assert!(!should_send_initial_bitfield(&[0, 0, 0]));
    }

    #[test]
    fn initial_bitfield_is_sent_when_any_piece_is_local() {
        assert!(should_send_initial_bitfield(&[0x80]));
        assert!(should_send_initial_bitfield(&[0, 0x01]));
    }

    #[test]
    fn build_hash_response_rejects_unknown_root() {
        let (tables, _root) = make_v2_tables(4, 64 * 1024);
        let bogus = [0xAAu8; 32];
        match build_hash_response(Some(&tables), bogus, 2, 0, 4, 0) {
            Message::HashReject { pieces_root, .. } => assert_eq!(pieces_root, bogus),
            _ => panic!("expected HashReject"),
        }
    }

    #[test]
    fn build_hash_response_hybrid_torrent_served_via_tables() {
        // Hybrid torrents use V1Sha1 for piece verification, but must still
        // serve BEP-52 hash requests. Simulate this by passing Some(tables)
        // while having a V1Sha1 verifier — the function no longer looks at
        // the verifier at all
        let (tables, root) = make_v2_tables(4, 64 * 1024);
        let base = tables[0].piece_layer_base();
        let length = tables[0].piece_layer_padded_len();
        let expected = tables[0].serve_full_piece_layer().unwrap();

        match build_hash_response(Some(&tables), root.0, base, 0, length, 0) {
            Message::Hashes { hashes, .. } => {
                assert_eq!(
                    hashes.as_ref(),
                    expected.as_slice(),
                    "hybrid torrent should serve piece layer via explicit tables"
                );
            }
            other => panic!("expected Hashes for hybrid torrent, got {other:?}"),
        }
    }

    #[test]
    fn build_hash_response_rejects_when_no_tables() {
        // Pure-v1 torrent: tables = None → always HashReject
        match build_hash_response(None, [0u8; 32], 0, 0, 1, 0) {
            Message::HashReject { .. } => {}
            _ => panic!("expected HashReject for None tables"),
        }
    }

    #[test]
    fn build_hash_response_rejects_partial_request() {
        let (tables, root) = make_v2_tables(4, 64 * 1024);
        // proof_layers > 0 → reject
        match build_hash_response(Some(&tables), root.0, 2, 0, 4, 1) {
            Message::HashReject { .. } => {}
            _ => panic!("expected HashReject for proof_layers != 0"),
        }
        // wrong base_layer → reject
        match build_hash_response(Some(&tables), root.0, 99, 0, 4, 0) {
            Message::HashReject { .. } => {}
            _ => panic!("expected HashReject for wrong base_layer"),
        }
    }

    #[test]
    fn build_hash_response_rejects_v1_verifier() {
        // Passing None tables → HashReject regardless of other params
        match build_hash_response(None, [0u8; 32], 0, 0, 1, 0) {
            Message::HashReject { .. } => {}
            _ => panic!("expected HashReject for None (v1-only) tables"),
        }
    }

    #[test]
    fn choke_slots_respect_limit_and_optimistic() {
        let candidates: Vec<(u32, u64)> =
            vec![(1, 100), (2, 500), (3, 300), (4, 50), (5, 400), (6, 200)];
        let set = select_unchoked(candidates.clone(), None, 4);
        assert_eq!(set.len(), 4);
        assert!(set.contains(&2) && set.contains(&5) && set.contains(&3) && set.contains(&6));

        let set = select_unchoked(candidates, Some(4), 4);
        assert_eq!(set.len(), 4);
        assert!(set.contains(&4), "optimistic peer must hold a slot");
        assert!(set.contains(&2) && set.contains(&5) && set.contains(&3));
        assert!(!set.contains(&6), "slowest regular peer loses its slot");
    }

    #[test]
    fn optimistic_rotation_is_round_robin() {
        let c = [1u32, 3, 7];
        assert_eq!(next_optimistic(&c, None), Some(1));
        assert_eq!(next_optimistic(&c, Some(1)), Some(3));
        assert_eq!(next_optimistic(&c, Some(3)), Some(7));
        assert_eq!(next_optimistic(&c, Some(7)), Some(1));
        assert_eq!(next_optimistic(&[], Some(7)), None);
    }
}
