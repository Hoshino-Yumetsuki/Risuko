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
// SHA-1 hashing is now encapsulated in `PieceVerifier::V1Sha1`; the loop
// no longer hashes pieces directly
use tokio::sync::{mpsc, oneshot};
use tokio::time::{interval, MissedTickBehavior};

use super::core::{
    supports_v2_wire, Id20, Lengths, MerkleProofTable, PieceVerifier, TorrentMeta,
    ValidatedTorrentMetaV1Info,
};
use super::peer::{connect_with_utp_fallback, PeerCommand, PeerEvent, SpawnPeer};
use super::piece::{ChunkTracker, PieceTracker};
use super::storage::{FilesystemStorage, StorageBackend};
use super::tracker::{announce as tracker_announce, AnnounceEvent, AnnounceRequest};
use super::utp::UtpSocket;
use super::wire::extended::{ut_metadata_data, ut_metadata_type, ExtHandshake, EXT_HANDSHAKE_ID};
use super::wire::{Message, MessageEncoder};

pub use stats::{
    AggregatedLiveStats, LiveStats, PeerSnapshot, Snapshot, SpeedSample, TorrentStats,
};

/// Initial per-peer concurrent 16 KiB chunk request count. Each peer's pipeline grows
/// adaptively from this floor up to [`UB_MAX_OUTSTANDING_PER_PEER`] as it
/// proves it can keep up. Starting low prevents one slow peer from
/// hoarding a large block of pieces (which would later trigger a
/// REQUEST_TIMEOUT reclaim cascade and oscillate the global download
/// rate). The cap is enforced via the per-`Peer.max_outstanding` field;
/// the session-level setting overrides this default unconditionally.
const DEFAULT_MAX_OUTSTANDING_PER_PEER: usize = 6;
/// Upper bound on the per-peer adaptive pipeline depth. With 16 KiB chunks this caps a single
/// peer at 4 MiB in flight — enough to saturate any realistic link RTT
/// while still allowing the chunk tracker to spread work across peers
const UB_MAX_OUTSTANDING_PER_PEER: usize = 256;
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

/// Disconnect peers that have not sent us any wire message for this long.
/// The reader task has no per-read timeout post-handshake (a deliberate
/// choice to keep the hot-path zero-cost), so without an idle eviction in
/// the torrent loop a peer killed by a NAT idle expiry, an ISP-throttled
/// black-holed TCP session, or a peer that crashed without sending RST
/// will sit in `peers` forever, occupying one of the `max_peers` slots
/// and excluding fresh tracker / DHT addresses from being dialled. Once
/// enough peers reach this state — typical on long-running downloads
/// against swarms with many flaky NAT-ed leechers — `peers.len() ==
/// max_peers` and the global download rate collapses even while the UI
/// still reports a healthy peer count. 180 s is 2× our 90 s KeepAlive
/// cadence so a healthy peer that only ever sends KeepAlive (e.g. two
/// idle seeders sharing nothing) is never evicted in error
const PEER_IDLE_TIMEOUT: Duration = Duration::from_secs(180);

/// A peer marked `snubbing` (one of its chunk requests timed out) is
/// excluded from new request allocation until it delivers any Piece. If
/// it never delivers, the soft back-off becomes a permanent slot leak —
/// see `PEER_IDLE_TIMEOUT` for the same effect via a different path. We
/// hard-disconnect peers that stay snubbed for this long so the slot is
/// recycled to a peer that can actually serve us bytes
const SNUB_EVICTION_TIMEOUT: Duration = Duration::from_secs(60);

/// Per-peer message id we advertise for the BEP-9 `ut_metadata` extension.
/// Peers send `Extended { ext_id: OUR_UT_METADATA_ID, .. }` to request a
/// 16 KiB chunk of our raw `info` dict
const OUR_UT_METADATA_ID: u8 = 3;
/// Per-peer message id we advertise for `ut_pex` (BEP-11). Peers send
/// `Extended { ext_id: OUR_UT_PEX_ID, .. }` carrying gossiped swarm members;
/// we parse the `added`/`added6` fields and feed them into the dial path
/// (see the Extended handler)
const OUR_UT_PEX_ID: u8 = 4;
/// BEP-9 metadata piece size: every ut_metadata DATA carries up to one
/// 16 KiB block of the info dict, except possibly the last
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
    /// Per-piece verifier strategy chosen at session-attach time. Hybrid
    /// and pure-v1 torrents use SHA-1; pure-v2 torrents use SHA-256
    /// Merkle subtree verification
    pub verifier: PieceVerifier,
    /// Whether multi-file torrents are wrapped in a `<root>/<name>/`
    /// subfolder (the BitTorrent default). When `false`, files are
    /// written directly under `root_dir`. Carried on `ManagedTorrent`
    /// so `Session::delete(with_files=true)` can locate the right paths
    pub create_subfolder: bool,
    /// Shared µTP (BEP-29) endpoint for this session. When present, outbound
    /// dials that fail over TCP retry over µTP. `None` disables µTP.
    pub utp: Option<Arc<UtpSocket>>,
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
    /// Mirror of `TorrentInit::create_subfolder` so `Session::delete`
    /// can decide whether the on-disk layout is grouped or flat.
    pub create_subfolder: bool,
    /// Resolved per-torrent root directory: the folder that directly
    /// contains the torrent's files on disk. For multi-file grouped
    /// layouts this already includes the torrent name. Mirrors
    /// `TorrentInit::root_dir` so `Session::delete(with_files=true)`
    /// can target the actual output location rather than the session
    /// default `output_dir` (per-torrent `opts.output_folder` overrides).
    pub root_dir: PathBuf,
    /// Atomically updated by `torrent_loop` after Merkle tables are built.
    /// Starts as `init.advertise_v2`; clamped to `false` when `serve_v2_layers`
    /// turns out to be false so inbound handshakes never assert the v2 bit
    /// without valid Merkle proof tables
    pub advertise_v2: Arc<AtomicBool>,
    /// Pre-serialized BEP-10 extended handshake — encoded once at spawn
    /// time from the torrent's `info_bytes` length and our extension ids.
    /// Per-peer builder for the BEP-10 extended handshake bytes. Captures
    /// this torrent's metadata size and our extension ids; takes the peer's
    /// IP address per call so we can populate `yourip`. The accept loop
    /// hands a clone of this builder to the connection layer so inbound
    /// peers receive our extended handshake on the same async frame as
    /// the BT handshake exchange completes (and with `yourip` set)
    pub ext_handshake_builder: crate::peer::ExtHandshakeBuilder,
    pub(crate) cmd_tx: mpsc::Sender<TorrentCommand>,
    pub(crate) stats: Arc<Mutex<TorrentStats>>,
}

impl ManagedTorrent {
    pub fn info_hash(&self) -> Id20 {
        self.info_hash
    }
    /// Returns the v2 (SHA-256) info-hash if the loaded metadata advertises
    /// one (hybrid or pure-v2 torrents). `None` for pure-v1
    pub fn info_hash_v2(&self) -> Option<crate::core::hash::Id32> {
        self.metadata.load().as_ref().and_then(|m| m.info_hash_v2)
    }
    /// Returns the BEP-52 metadata version classification: `"v1"`,
    /// `"v2"`, or `"hybrid"`. `None` if metadata not yet loaded
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
    /// Per-peer adaptive pipeline cap
    /// `DefaultBtInteractive::receiveMessages`: starts at
    /// [`DEFAULT_MAX_OUTSTANDING_PER_PEER`], doubles when this peer
    /// fulfils ≥ 25 % of its currently-permitted slots in a window, and
    /// is capped at [`UB_MAX_OUTSTANDING_PER_PEER`]. Slow peers stay near
    /// the floor so their unfulfilled requests never dominate the chunk
    /// tracker, eliminating the REQUEST_TIMEOUT reclaim cascades that
    /// otherwise oscillate the global download rate
    max_outstanding: usize,
    /// Number of `Piece` messages received from this peer since the last
    /// time `max_outstanding` was grown. Used to decide when the next
    /// doubling fires (see [`maybe_grow_pipeline`])
    received_since_grow: usize,
    /// "Snubbing" flag. Set when one of this peer's chunk
    /// requests times out; cleared as soon as the peer delivers any
    /// `Piece`. While set, [`drive_peer`] issues no new requests to it,
    /// so the peer's stuck slots drain back into the pool naturally
    /// instead of being immediately refilled. Without this, a peer that
    /// briefly stalls keeps reclaiming + refilling chunks every
    /// REQUEST_TIMEOUT, dominating the chunk tracker and starving fast
    /// peers — the slow-decay cause of the
    /// "speed great at first, then degrades over minutes" pattern
    snubbing: bool,
    /// Instant we last received any wire message from this peer. Updated
    /// on every `PeerEvent::Message` arrival; consulted by the tick to
    /// evict peers idle past [`PEER_IDLE_TIMEOUT`]. Initialised on
    /// adoption / handshake so a peer is never evicted before it has had
    /// a chance to send anything
    last_recv: Instant,
    /// Instant the `snubbing` flag was last set, or `None` if not
    /// snubbing. Consulted by the tick to evict peers that stay snubbed
    /// past [`SNUB_EVICTION_TIMEOUT`] (otherwise a peer whose requests
    /// keep timing out occupies a slot indefinitely without serving any
    /// bytes)
    snub_since: Option<Instant>,
    /// Per-peer message id the remote advertised for `ut_metadata` in its
    /// BEP-10 extended handshake. `None` until that handshake is received.
    /// We never *initiate* a `ut_metadata` request from the torrent loop
    /// (info is already loaded) but record the id so the responder can
    /// echo it back on DATA / REJECT replies
    their_ut_metadata_id: Option<u8>,
}

/// Result of an off-runtime piece write + verify pass
///
/// Verification (`PieceVerifier::V1Sha1` or `V2Merkle`) runs inside the
/// per-piece spawned task so the torrent loop never blocks on hashing —
/// it only consumes the boolean outcome. Sending the full piece bytes
/// through this channel and re-spawning a `spawn_blocking` from the loop
/// would serialize every piece completion through the select arm and
/// stall all peer / tracker / tick events for the duration of each hash
struct VerifyResult {
    piece_index: u32,
    /// Set when the disk write for this piece returned an error. The
    /// torrent loop must not mark the piece local in that case — the data
    /// on disk is incomplete and the piece must be re-requested
    write_failed: bool,
    /// `true` when the verifier accepted the piece bytes
    verify_ok: bool,
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
    advertise_v2_flag: Arc<AtomicBool>,
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
    // TCP connect can retry over µTP.
    let utp = init.utp.clone();
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
                            log::warn!("hybrid torrent {info_hash}: could not build Merkle table for serving: {e}");
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
    // True only when we have valid Merkle tables: gates BEP-52 HASH_REQUEST
    // serving AND v2/truncated info-hash announcement on trackers.
    // A failed from_layer_bytes build leaves hash_tables = None so we must
    // not advertise v2 capability in that case.
    let serve_v2_layers = supports_v2 && hash_tables.is_some();
    // Clamp to actual capability now that hash_tables is known. Updates the
    // shared handle field so inbound connection handling sees the same value
    let advertise_v2 = init.advertise_v2 && serve_v2_layers;
    advertise_v2_flag.store(advertise_v2, Ordering::Relaxed);
    // Per-peer pipeline depth bounds. When the session explicitly sets
    // `max_outstanding_per_peer` we honour it as a fixed value (legacy
    // behaviour, useful for benchmarks / debugging); otherwise we use
    // an adaptive range \u2014 start at the floor and grow per peer based
    // on observed delivery rate up to the cap.
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
    let storage = Arc::new(FilesystemStorage::new(&info, &init.root_dir));
    if let Err(e) = storage.preallocate().await {
        log::warn!("preallocate failed for {info_hash}: {e}");
    }
    let mut piece_tracker = PieceTracker::new(lengths);
    let mut chunk_tracker = ChunkTracker::new(lengths);
    let mut piece_assemblies: HashMap<u32, PieceAssembly> = HashMap::new();
    scan_existing_pieces(&info, &verifier, &storage, &lengths, &mut piece_tracker).await;
    {
        let mut s = stats.lock();
        s.progress_bytes = completed_bytes(&piece_tracker, &lengths);
        s.file_progress = compute_file_progress(&piece_tracker, &lengths, storage.layout());
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
    // (with dedup + cap enforcement). DHT feeds peers via the AddPeer command.
    let (peer_src_tx, mut peer_addr_rx) = mpsc::channel::<SocketAddr>(256);
    spawn_tracker_pollers(
        peer_src_tx.clone(),
        collect_trackers(&init.meta),
        announce_hashes,
        our_peer_id,
        listen_port,
        Arc::clone(&stats),
    );

    // Pre-serialize the BEP-10 extended handshake once. The connection
    // layer ships this on the wire in the same async frame that just
    // completed the BT handshake, eliminating tokio task hops between
    // "BT handshake done" and "ext handshake on the wire". Some peers RST
    // the connection if our follow-up doesn't arrive within their grace
    // window. The builder is invoked per-peer so each `yourip` field
    // matches the dialed peer's address
    let initial_ext_handshake_builder: crate::peer::ExtHandshakeBuilder = {
        let metadata_size = info_bytes.len() as u64;
        std::sync::Arc::new(move |peer_ip: std::net::IpAddr| {
            let hs =
                ExtHandshake::new_outgoing(OUR_UT_METADATA_ID, OUR_UT_PEX_ID, Some(metadata_size))
                    .with_yourip(peer_ip);
            MessageEncoder::encode(&Message::Extended {
                ext_id: EXT_HANDSHAKE_ID,
                payload: hs.encode(),
            })
        })
    };

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
    // Send a BEP-3 KeepAlive (zero-length message) to every peer roughly
    // every 90 s. Peers — most clients default to a 2-min idle timeout —
    // will close us otherwise, which is the dominant reason a fully-seeded
    // torrent gradually loses every connection (no Request/Piece traffic
    // flows between two seeders, so without an explicit liveness frame
    // the TCP session looks dead from their side).
    let mut last_keepalive = Instant::now();
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
                        spawn_outbound_peer(
                            torrent_id,
                            pid,
                            addr,
                            info_hash,
                            our_peer_id,
                            peer_event_tx.clone(),
                            encryption,
                            advertise_v2,
                            Some(initial_ext_handshake_builder.clone()),
                            utp.clone(),
                        );
                    }
                }
                TorrentCommand::AddInboundPeer { addr, cmd_tx, event_rx } => {
                    if !paused
                        && known_addrs.insert(addr)
                        && peers.len() < max_peers
                    {
                        let pid = next_pid; next_pid += 1;
                        adopt_inbound_peer(pid, addr, cmd_tx, event_rx, peer_event_tx.clone(), &mut peers, &lengths, &mut piece_tracker, pipeline_floor).await;
                    }
                }
                TorrentCommand::Pause(ack) => {
                    paused = true;
                    for (_, p) in peers.drain() {
                        let _ = p.cmd_tx.send(PeerCommand::Disconnect).await;
                    }
                    pending_dials.clear();
                    known_addrs.clear();
                    // Release the cached file descriptors
                    if let Err(e) = storage.close_handles().await {
                        log::warn!("failed to close storage handles on pause: {e}");
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
                    spawn_outbound_peer(
                        torrent_id,
                        pid,
                        addr,
                        info_hash,
                        our_peer_id,
                        peer_event_tx.clone(),
                        encryption,
                        advertise_v2,
                        Some(initial_ext_handshake_builder.clone()),
                        utp.clone(),
                    );
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
                    &peer_src_tx,
                    &verify_tx,
                    &verifier,
                    &info_bytes,
                    hash_tables.as_deref().map(|v| &**v),
                    pipeline_floor,
                    pipeline_cap,
                    max_peers,
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
                // even while peer count stays constant.
                let reclaimed = chunk_tracker.reclaim_stale(REQUEST_TIMEOUT);
                if !reclaimed.is_empty() {
                    let mut unblocked_pieces: HashSet<u32> = HashSet::new();
                    // Snubbing: mark every peer that owned a
                    // reclaimed chunk so `drive_peer` stops issuing new
                    // requests to it. The flag clears as soon as the peer
                    // delivers any `Piece` (in `process_peer_event`) so
                    // healthy peers recover instantly while a genuinely
                    // stuck peer parks until its TCP read timeout
                    // disconnects it. Crucially this does NOT shrink the
                    // cap: a one-way shrink-on-reclaim ratchet decays
                    // every peer toward the floor over minutes and is the
                    // cause of the "fast at first, slow after a while"
                    // pattern - each reclaim cycle halves the offender,
                    // and a single 8 s blip permanently demotes a peer
                    // that was previously fine. Snubbing punishes only
                    // the actually-stuck peers, transiently
                    for r in &reclaimed {
                        if let Some(p) = peers.get_mut(&r.peer) {
                            if !p.snubbing {
                                p.snubbing = true;
                                p.snub_since = Some(Instant::now());
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
                // Evict peers that have gone silent or stayed snubbed for
                // too long. Without this sweep their slot is held until
                // the (currently absent) reader-side timeout fires — i.e.
                // never — so peers leak permanently and `peers.len()`
                // saturates at `max_peers`, blocking fresh tracker / DHT
                // addresses. This is the dominant cause of "download speed
                // is great at first then collapses to 0 after a while":
                // each silent / snubbed peer parks a slot, and the
                // reclaim/snub mechanism alone can't free it because the
                // peer never delivers anything to clear `snubbing`.
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
                            piece_tracker.remove_peer_bitfield(&p.bitfield);
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
                    drive_requests(&mut peers, &mut piece_tracker, &mut chunk_tracker).await;
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
    type PeerCmdRegistry = StdMutex<HashMap<(usize, u32), (mpsc::Sender<PeerCommand>, SocketAddr)>>;
    static REG: Lazy<PeerCmdRegistry> = Lazy::new(|| StdMutex::new(HashMap::new()));
    pub fn put(torrent_id: usize, pid: u32, tx: mpsc::Sender<PeerCommand>, addr: SocketAddr) {
        REG.lock().unwrap().insert((torrent_id, pid), (tx, addr));
    }
    pub fn take(torrent_id: usize, pid: u32) -> Option<(mpsc::Sender<PeerCommand>, SocketAddr)> {
        REG.lock().unwrap().remove(&(torrent_id, pid))
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_outbound_peer(
    torrent_id: usize,
    pid: u32,
    addr: SocketAddr,
    info_hash: Id20,
    our_peer_id: Id20,
    event_tx: mpsc::Sender<(u32, PeerEvent)>,
    encryption: crate::peer::EncryptionPolicy,
    advertise_v2: bool,
    ext_handshake_builder: Option<crate::peer::ExtHandshakeBuilder>,
    utp: Option<Arc<UtpSocket>>,
) {
    tokio::spawn(async move {
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
                peer_registry::put(torrent_id, pid, handle.tx.clone(), handle.addr);
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
) {
    // Install the peer directly in the per-loop `peers` map. We deliberately
    // do *not* route through the shared `peer_registry` (used for outbound
    // dials); the registry is keyed by `(torrent_id, pid)` and torrent ids
    // are not unique across sessions running in the same process — using it
    // for both directions would cause cross-session take/put collisions in
    // any environment that hosts multiple sessions (notably integration
    // tests, but also future per-user multi-session deployments).
    //
    // The ext-handshake is already on the wire (the connection layer wrote
    // it inline before pushing the `Handshook` event), so we only need to
    // emit the optional bitfield + unchoke here. `process_peer_event`'s
    // `Handshook` arm short-circuits on `peers.contains_key(pid)`, so the
    // forwarded event is a benign no-op
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
            received_since_grow: 0,
            snubbing: false,
            last_recv: Instant::now(),
            snub_since: None,
            their_ut_metadata_id: None,
        },
    );
    let bf = piece_tracker.bitfield();
    if should_send_initial_bitfield(&bf) {
        let _ = cmd_tx
            .send(PeerCommand::Send(Message::Bitfield(Bytes::from(bf))))
            .await;
    }
    let _ = cmd_tx.send(PeerCommand::Send(Message::Unchoke)).await;
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
    peer_src_tx: &mpsc::Sender<SocketAddr>,
    verify_tx: &mpsc::Sender<VerifyResult>,
    verifier: &PieceVerifier,
    info_bytes: &Arc<Vec<u8>>,
    hash_tables: Option<&[MerkleProofTable]>,
    pipeline_floor: usize,
    pipeline_cap: usize,
    max_peers: usize,
) -> bool {
    // Return value: `true` if the caller should immediately kick the peer
    // request pipeline. Set for events that can free an outstanding slot
    // (Piece) or unblock requests (Unchoke, Bitfield, Have)
    let mut kick = false;
    match ev {
        PeerEvent::Handshook { encrypted, .. } => {
            if !peers.contains_key(&pid) {
                if let Some((cmd_tx, registry_addr)) = peer_registry::take(torrent_id, pid) {
                    // Move from pending dial to live peer. The registry is
                    // the authoritative source of `addr` because
                    // `pending_dials` may have been cleared by Pause/Stop
                    // while the connect+handshake was in flight; the
                    // registry entry is only ever written by the spawn
                    // task that actually completed the TCP connect.
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
                    log::debug!(
                        "peer connected: {addr} (encrypted={encrypted}, peers={}/{max_peers})",
                        peers.len() + 1
                    );
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
                            received_since_grow: 0,
                            snubbing: false,
                            last_recv: Instant::now(),
                            snub_since: None,
                            their_ut_metadata_id: None,
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
            // Refresh per-peer liveness on every wire message (including
            // KeepAlive / Choke / Have, not just Piece). The torrent loop's
            // tick uses this to evict peers idle past PEER_IDLE_TIMEOUT
            // and recycle their slot to a fresh dial \u2014 see the eviction
            // sweep in the `tick.tick()` arm
            peer.last_recv = Instant::now();
            if log::log_enabled!(target: "diag", log::Level::Debug) {
                let kind = match &msg {
                    Message::KeepAlive => "KeepAlive".to_string(),
                    Message::Choke => "Choke".to_string(),
                    Message::Unchoke => "Unchoke".to_string(),
                    Message::Interested => "Interested".to_string(),
                    Message::NotInterested => "NotInterested".to_string(),
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
                log::debug!(
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
                    // Adaptive pipeline grow: count successful
                    // deliveries in the current window, double the per-peer
                    // cap when 25 % of the cap has landed (cap at
                    // pipeline_cap). Doing this off the steady-state Piece
                    // path keeps fast peers ramping toward saturation while
                    // slow peers stay near pipeline_floor and never hoard
                    // a large share of the chunk tracker
                    peer.received_since_grow += 1;
                    if peer.max_outstanding < pipeline_cap
                        && peer.received_since_grow * 4 >= peer.max_outstanding
                    {
                        peer.max_outstanding = (peer.max_outstanding * 2).min(pipeline_cap);
                        peer.received_since_grow = 0;
                    }
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
                        }
                    } else if ext_id == OUR_UT_METADATA_ID {
                        serve_ut_metadata(peer, &payload, info_bytes);
                    } else if ext_id == OUR_UT_PEX_ID {
                        // A connected peer (often a seeder) gossips other swarm members
                        if let Some((v4, v6)) = super::wire::extended::parse_ut_pex(&payload) {
                            for addr in v4.into_iter().chain(v6) {
                                let _ = peer_src_tx.try_send(addr);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        PeerEvent::Disconnected { reason } => {
            // Clear from either in-flight or live, and release the address for future
            // retries (otherwise a single drop permanently blacklists the peer)
            let addr = pending_dials
                .remove(&pid)
                .or_else(|| peers.get(&pid).map(|p| p.addr));
            if let Some(a) = addr {
                // Log the disconnect cause at debug so the per-peer error —
                // typically the MSE handshake outcome for unreachable /
                // encryption-only peers — is visible to operators
                // troubleshooting "0 KB/s" reports without us having to
                // re-deduce it from "trying mse" lines alone
                log::debug!("peer {a} disconnected: {reason}");
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
                    let should_drop = piece_assemblies
                        .get(&piece_idx)
                        .is_none_or(|a| a.received_chunks.is_empty());
                    if should_drop {
                        piece_assemblies.remove(&piece_idx);
                    }
                }
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
        log::warn!(
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
        piece_tracker.set_local(vpi, true);
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
        s.progress_bytes = completed_bytes(piece_tracker, lengths);
        s.file_progress = compute_file_progress(piece_tracker, lengths, storage.layout());
        s.finished = piece_tracker.is_complete();
    } else {
        log::debug!("piece {} verify failed", vr.piece_index);
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

async fn send_interested_if_useful(peer: &mut Peer, piece_tracker: &mut PieceTracker) {
    let useful = piece_tracker.choose_piece(&peer.bitfield).is_some();
    let set_bits: u32 = peer.bitfield.iter().map(|x| x.count_ones()).sum();
    log::debug!(
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

/// Send a `Cancel` message to every peer (other than `except_pid`) that has
/// the chunk `(index, begin, length)` in its outstanding queue, and remove
/// the entry from each peer's local outstanding Vec. Best-effort: if a
/// peer's command channel is full or closed we skip it (the chunk will
/// arrive and be dedup-dropped, no worse than today). This is the only
/// thing that keeps endgame mode from saturating downstream with duplicate
/// blocks at the very end of a download.
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
    let _ = info; // kept for symmetry with previous signature; verifier owns v1/v2 dispatch
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
) {
    for url in trackers {
        for info_hash in &info_hashes {
            let tx = tx.clone();
            let url = url.clone();
            let info_hash = *info_hash;
            let stats = Arc::clone(&stats);
            tokio::spawn(async move {
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
                            log::debug!("tracker {url} failed: {e}");
                            tokio::time::sleep(Duration::from_secs(120)).await;
                        }
                    }
                    event = AnnounceEvent::None;
                }
            });
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
}
