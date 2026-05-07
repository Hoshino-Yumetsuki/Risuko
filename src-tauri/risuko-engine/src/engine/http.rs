use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::Arc;

use bytes::Bytes;
use futures_util::StreamExt;
use risuko_http::header::{
    HeaderMap, HeaderName, HeaderValue, ACCEPT_ENCODING, ACCEPT_RANGES, CONTENT_ENCODING,
    CONTENT_LENGTH, CONTENT_RANGE, ETAG, IF_MATCH, LAST_MODIFIED, RANGE, TRANSFER_ENCODING,
};
use risuko_http::Client;
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use super::speed_limiter::{parse_speed_limit, SpeedLimiter};

const PART_SUFFIX: &str = ".part";
/// Default minimum segment size when `min-split-size` is not set: 1 MiB.
/// Don't split below this — small files get a single connection.
const DEFAULT_MIN_SPLIT_SIZE: u64 = 1024 * 1024;
/// Max retries per piece on transient errors
const CHUNK_MAX_RETRIES: u32 = 5;
/// Resume granularity: every multi-chunk download is divided into 1 MiB pieces.
/// Workers pull pieces off a shared queue (work-stealing for free) and resume
/// preserves per-piece byte progress so a SIGKILL never loses more than the
/// in-flight bytes of one piece per worker.
///
/// Public so integration tests can size payloads in piece-boundary terms
/// instead of hard-coding 1 MiB
pub const PIECE_SIZE: u64 = 1024 * 1024;
/// Sidecar format version. Bump when the on-disk schema changes
const META_VERSION: u32 = 2;
/// How often the periodic save task snapshots piece progress to disk.
const META_SAVE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);
/// Error substring returned when a stale .part was removed
const STALE_PART_REMOVED: &str = "stale partial file removed";
/// Exponential moving average smoothing factor for speed reporting
const SPEED_EMA_ALPHA: f64 = 0.3;
/// Suffix for per-chunk resume metadata file
use super::CHUNK_META_SUFFIX;

/// Inclusive byte range [start, end]
#[derive(Debug, Clone, Copy)]
struct ChunkRange {
    start: u64,
    end: u64,
}

impl ChunkRange {
    fn new(start: u64, end: u64) -> Self {
        debug_assert!(end >= start);
        Self { start, end }
    }

    fn to_range_header_value(self) -> String {
        format!("bytes={}-{}", self.start, self.end)
    }
}

/// Outcome from a single stream call. Per-piece progress is tracked via the
/// shared `piece_completed: Arc<AtomicU32>` that the writer task updates as
/// bytes hit disk — the caller reads that atomic instead of relying on a
/// returned byte count.
struct StreamOutcome {
    error: Option<String>,
}

/// Per-piece resume entry. Sparse: only pieces with `c > 0` are persisted.
#[derive(serde::Serialize, serde::Deserialize)]
struct PieceProgress {
    /// piece index
    i: u32,
    /// bytes completed in this piece (0 < c <= piece length)
    c: u32,
}

/// Resume metadata for a multi-piece download. JSON sidecar next to the
/// `.part` file. Versioned: incompatible versions are discarded and the
/// download restarts from scratch (the file is still pre-allocated, so the
/// kernel page cache may have warm data, but we re-issue every Range).
#[derive(serde::Serialize, serde::Deserialize)]
struct ChunkMeta {
    version: u32,
    content_length: u64,
    piece_size: u32,
    etag: Option<String>,
    /// Sparse list of pieces with progress. A piece is fully done when
    /// `c == piece_length`. Pieces not listed start fresh at 0.
    pieces: Vec<PieceProgress>,
}

fn chunk_meta_path(part_path: &Path) -> PathBuf {
    let mut p = part_path.as_os_str().to_owned();
    p.push(CHUNK_META_SUFFIX);
    PathBuf::from(p)
}

fn save_piece_meta(
    part_path: &Path,
    queue: &PieceQueue,
    content_length: u64,
    etag: &Option<String>,
) {
    let pieces: Vec<PieceProgress> = queue
        .pieces
        .iter()
        .enumerate()
        .filter_map(|(i, p)| {
            let c = p.completed.load(Ordering::Relaxed);
            if c == 0 {
                None
            } else {
                Some(PieceProgress { i: i as u32, c })
            }
        })
        .collect();
    let meta = ChunkMeta {
        version: META_VERSION,
        content_length,
        piece_size: PIECE_SIZE as u32,
        etag: etag.clone(),
        pieces,
    };
    let path = chunk_meta_path(part_path);
    if let Ok(json) = serde_json::to_string(&meta) {
        let _ = fs::write(&path, json);
    }
}

fn load_piece_meta(
    part_path: &Path,
    content_length: u64,
    etag: &Option<String>,
) -> Option<ChunkMeta> {
    let path = chunk_meta_path(part_path);
    let data = fs::read_to_string(&path).ok()?;
    let meta: ChunkMeta = serde_json::from_str(&data).ok()?;
    if meta.version != META_VERSION
        || meta.content_length != content_length
        || meta.piece_size != PIECE_SIZE as u32
    {
        let _ = fs::remove_file(&path);
        return None;
    }

    // Verify the .part file exists with the expected pre-allocated size
    match fs::metadata(part_path) {
        Ok(m) if m.len() == content_length => {}
        _ => {
            let _ = fs::remove_file(&path);
            return None;
        }
    }

    // Saved ETag without a current probe ETag means we can't verify integrity
    if meta.etag.is_some() && etag.is_none() {
        let _ = fs::remove_file(&path);
        return None;
    }
    if let (Some(saved), Some(current)) = (&meta.etag, etag) {
        if saved != current {
            let _ = fs::remove_file(&path);
            return None;
        }
    }
    // No freshness validator on either side: trusting the partial would let
    // a server that silently changed the resource produce a corrupt final
    // file. Reject resume and start over
    if meta.etag.is_none() && etag.is_none() {
        let _ = fs::remove_file(&path);
        return None;
    }

    Some(meta)
}

fn delete_chunk_meta(part_path: &Path) {
    let _ = fs::remove_file(chunk_meta_path(part_path));
}

// === Piece queue: shared work pool for the worker tasks ===
//
// Each piece is at most PIECE_SIZE bytes. Workers atomically claim a free
// piece (CAS state 0 -> 1), download it, then mark it done (state -> 2) or
// release it back to the pool on transient error (state -> 0, completed
// bytes preserved so the next claimant resumes mid-piece).
//
// This naturally implements work stealing: fast workers pull more pieces;
// slow or idle workers simply pull fewer. There is no explicit "steal from
// peer X" call because nothing is owned long-term.

/// Free / in-flight / done state encoded in a single byte for cheap CAS.
const PIECE_FREE: u8 = 0;
const PIECE_INFLIGHT: u8 = 1;
const PIECE_DONE: u8 = 2;

struct Piece {
    /// Absolute byte offset in the output file.
    offset: u64,
    /// Piece length in bytes (last piece may be < PIECE_SIZE).
    length: u32,
    /// Bytes confirmed flushed to disk so far. Monotonically increasing
    /// because positioned writes are issued in order from the writer task.
    /// Wrapped in Arc so the writer task can hold a clone independently.
    completed: Arc<AtomicU32>,
    /// Lifecycle state — see PIECE_* constants.
    state: AtomicU8,
}

struct PieceQueue {
    pieces: Vec<Piece>,
    /// Hint for the next free piece, to avoid rescanning from 0 every claim.
    next_hint: AtomicUsize,
}

impl PieceQueue {
    fn new(content_length: u64) -> Self {
        debug_assert!(content_length > 0);
        let n = content_length.div_ceil(PIECE_SIZE) as usize;
        let mut pieces = Vec::with_capacity(n);
        for i in 0..n {
            let offset = i as u64 * PIECE_SIZE;
            let length = std::cmp::min(PIECE_SIZE, content_length - offset) as u32;
            pieces.push(Piece {
                offset,
                length,
                completed: Arc::new(AtomicU32::new(0)),
                state: AtomicU8::new(PIECE_FREE),
            });
        }
        Self {
            pieces,
            next_hint: AtomicUsize::new(0),
        }
    }

    /// Try to claim the next free piece, scanning circularly from the hint.
    /// Returns the piece index, or None if the queue is exhausted.
    fn claim_next(&self) -> Option<usize> {
        let n = self.pieces.len();
        if n == 0 {
            return None;
        }
        let start = self.next_hint.load(Ordering::Relaxed) % n;
        for di in 0..n {
            let i = (start + di) % n;
            let p = &self.pieces[i];
            if p.state
                .compare_exchange(
                    PIECE_FREE,
                    PIECE_INFLIGHT,
                    Ordering::AcqRel,
                    Ordering::Relaxed,
                )
                .is_ok()
            {
                // Move hint past this slot. Best-effort — racy stores are fine.
                self.next_hint.store(i + 1, Ordering::Relaxed);
                return Some(i);
            }
        }
        None
    }

    /// Return a piece to the pool with its `completed` count preserved so the
    /// next claimant resumes mid-piece via a smaller Range request.
    fn release(&self, idx: usize) {
        self.pieces[idx].state.store(PIECE_FREE, Ordering::Release);
        // Bias the hint backwards so this piece gets retried sooner.
        let cur = self.next_hint.load(Ordering::Relaxed);
        if idx < cur {
            self.next_hint.store(idx, Ordering::Relaxed);
        }
    }

    fn complete(&self, idx: usize) {
        self.pieces[idx].state.store(PIECE_DONE, Ordering::Release);
    }

    /// True when every piece is in the DONE state. Used to decide whether
    /// worker errors should be treated as fatal: a stranded retry-exhausted
    /// worker is recoverable as long as another worker eventually finished
    /// the piece
    fn is_finished(&self) -> bool {
        self.pieces
            .iter()
            .all(|p| p.state.load(Ordering::Acquire) == PIECE_DONE)
    }
}

/// Build a HTTP Client with common settings applied from options
///
/// `cookie_jar` is shared with the range-request client so both clients see
/// the same cookie store; passing `None` disables the cookie provider for
/// this client
fn build_client(
    options: &Map<String, Value>,
    cookie_jar: Option<std::sync::Arc<risuko_http::Jar>>,
) -> Result<Client, String> {
    build_client_inner(options, true, cookie_jar)
}

/// Build a client without automatic decompression — needed for range requests
/// where we must receive raw bytes at exact file offsets.
fn build_client_no_decompress(
    options: &Map<String, Value>,
    cookie_jar: Option<std::sync::Arc<risuko_http::Jar>>,
) -> Result<Client, String> {
    build_client_inner(options, false, cookie_jar)
}

/// aria2 default: 60s connect timeout when not configured
const DEFAULT_CONNECT_TIMEOUT_SECS: u64 = 60;
/// aria2 default for `--lowest-speed-limit-timeout`: 30s of below-threshold
/// transfer before the worker is considered stalled
const DEFAULT_LOWEST_SPEED_TIMEOUT_SECS: u64 = 30;

/// Configuration + signal for the stalled-transfer watchdog. `lowest_speed`
/// of 0 disables the watchdog entirely (matches aria2's default behavior of
/// never enforcing a floor)
#[derive(Clone)]
struct StallWatchdog {
    lowest_speed: u64,
    timeout: std::time::Duration,
    flag: Arc<AtomicBool>,
}

impl StallWatchdog {
    fn from_options(options: &Map<String, Value>) -> Self {
        let lowest_speed = options
            .get("lowest-speed-limit")
            .map(parse_speed_limit)
            .unwrap_or(0);
        let timeout = parse_duration_secs_option(
            options.get("lowest-speed-limit-timeout"),
            DEFAULT_LOWEST_SPEED_TIMEOUT_SECS,
        );
        Self {
            lowest_speed,
            timeout,
            flag: Arc::new(AtomicBool::new(false)),
        }
    }
}

/// Read a duration option (seconds) with sane fallback.
fn parse_duration_secs_option(v: Option<&Value>, default: u64) -> std::time::Duration {
    let secs = v
        .and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok()))
        })
        .unwrap_or(default);
    std::time::Duration::from_secs(secs)
}

/// Pull `checksum` from the options map and parse it. Empty string / missing
/// key disables verification (returns `Ok(None)`); a non-empty but malformed
/// value is rejected so users see the typo immediately rather than after a
/// long download.
fn parse_whole_checksum_option(
    options: &Map<String, Value>,
) -> Result<Option<super::hasher::WholeChecksum>, String> {
    match options.get("checksum").and_then(Value::as_str) {
        Some(s) if !s.trim().is_empty() => super::hasher::WholeChecksum::parse(s.trim()).map(Some),
        _ => Ok(None),
    }
}

fn parse_piece_checksums_option(
    options: &Map<String, Value>,
) -> Result<Option<super::hasher::PieceChecksums>, String> {
    match options.get("piece-checksums").and_then(Value::as_str) {
        Some(s) if !s.trim().is_empty() => super::hasher::PieceChecksums::parse(s.trim()).map(Some),
        _ => Ok(None),
    }
}

/// Hash `path` from disk in 1 MiB blocks and verify against `expected`.
/// Runs on the blocking pool so the async runtime stays responsive even on
/// multi-GB files.
async fn verify_whole_file(
    path: &Path,
    expected: &super::hasher::WholeChecksum,
) -> Result<(), String> {
    let path = path.to_path_buf();
    let expected = expected.clone();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        use std::io::Read;
        let mut f = std::fs::File::open(&path).map_err(|e| format!("open for verify: {e}"))?;
        let mut hasher = super::hasher::Hasher::new(expected.algo);
        let mut buf = vec![0u8; 1024 * 1024];
        loop {
            let n = f
                .read(&mut buf)
                .map_err(|e| format!("read for verify: {e}"))?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        let got = hasher.finalize_hex();
        if expected.matches(&got) {
            Ok(())
        } else {
            Err(format!(
                "checksum mismatch ({}): expected {}, got {}",
                expected.algo.name(),
                expected.hex,
                got
            ))
        }
    })
    .await
    .map_err(|e| format!("verify task panicked: {e}"))?
}

/// Verify each piece against `expected`. The piece layout must match
/// `PIECE_SIZE` (the engine's fixed multi-piece chunk size); aria2-style
/// per-piece lengths aren't carried in the option, so we fail loudly if the
/// hash count doesn't line up with `(content_length / PIECE_SIZE).ceil()`
async fn verify_piece_checksums(
    path: &Path,
    content_length: u64,
    expected: &super::hasher::PieceChecksums,
) -> Result<(), String> {
    let expected_count = content_length.div_ceil(PIECE_SIZE) as usize;
    if expected.hexes.len() != expected_count {
        return Err(format!(
            "piece-checksums count mismatch: got {} hashes for {} pieces",
            expected.hexes.len(),
            expected_count
        ));
    }
    let path = path.to_path_buf();
    let expected = expected.clone();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        use std::io::Read;
        let mut f = std::fs::File::open(&path).map_err(|e| format!("open for verify: {e}"))?;
        let piece_size = PIECE_SIZE as usize;
        let mut buf = vec![0u8; piece_size];
        for (i, want) in expected.hexes.iter().enumerate() {
            // Last piece may be short; read exactly the remaining bytes so we
            // don't hash any pre-allocated zero padding.
            let offset = i as u64 * PIECE_SIZE;
            let remaining = content_length.saturating_sub(offset) as usize;
            let take = remaining.min(piece_size);
            f.read_exact(&mut buf[..take])
                .map_err(|e| format!("read piece {i}: {e}"))?;
            let mut hasher = super::hasher::Hasher::new(expected.algo);
            hasher.update(&buf[..take]);
            let got = hasher.finalize_hex();
            if !want.eq_ignore_ascii_case(&got) {
                return Err(format!(
                    "piece {} checksum mismatch ({}): expected {}, got {}",
                    i,
                    expected.algo.name(),
                    want,
                    got
                ));
            }
        }
        Ok(())
    })
    .await
    .map_err(|e| format!("verify task panicked: {e}"))?
}

fn build_client_inner(
    options: &Map<String, Value>,
    decompress: bool,
    cookie_jar: Option<std::sync::Arc<risuko_http::Jar>>,
) -> Result<Client, String> {
    let ua = options
        .get("user-agent")
        .and_then(|v| v.as_str())
        .unwrap_or("Mozilla/5.0");

    let connect_timeout =
        parse_duration_secs_option(options.get("connect-timeout"), DEFAULT_CONNECT_TIMEOUT_SECS);

    let mut builder = Client::builder()
        .user_agent(ua)
        .redirect(risuko_http::redirect::Policy::limited(10))
        .connect_timeout(connect_timeout)
        // Per-request `timeout` option is intentionally NOT applied here:
        // chunked / long-streaming downloads must not be cut off by a wall
        // clock. Stalled-transfer detection is handled separately by the
        // `lowest-speed-limit` watchdog in `run_speed_tracker`
        .tcp_nodelay(true)
        // Long-lived chunk connections benefit from generous keepalive, and
        // a large idle pool keeps every chunk worker on its own TCP stream.
        .tcp_keepalive(std::time::Duration::from_secs(60))
        .pool_idle_timeout(std::time::Duration::from_secs(90))
        .pool_max_idle_per_host(64);

    if decompress {
        builder = builder.gzip(true).brotli(true).deflate(true);
    } else {
        // Range/chunk workers MUST stay on HTTP/1.1. HTTP/2 multiplexes every
        // "parallel" request onto a single TCP stream, collapsing aggregate
        // throughput from a single origin. aria2 makes the same choice.
        builder = builder.no_gzip().no_brotli().no_deflate().http1_only();
    }

    if let Some(proxy_url) = options
        .get("all-proxy")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        if let Ok(proxy) = risuko_http::Proxy::all(proxy_url) {
            builder = builder.proxy(proxy);
        }
    }

    // Cookie file parsing happens once in the caller (see `load_cookie_jar`)
    // so the decompressing and range clients share a single jar — otherwise
    // each call here would re-parse the file into a disjoint store and
    // requests issued via the range client wouldn't see cookies set on
    // responses received by the main client (and vice versa)
    if let Some(jar) = cookie_jar {
        builder = builder.cookie_provider(jar);
    }

    builder
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {e}"))
}

/// Parse the `load-cookies` file (Netscape format) once and return a shared
/// `Jar`. Returns `None` when the option is unset/blank, the file is missing,
/// or parsing fails — all of those conditions are non-fatal and logged here
/// so the call sites stay simple
fn load_cookie_jar(options: &Map<String, Value>) -> Option<std::sync::Arc<risuko_http::Jar>> {
    let cookies_path = options
        .get("load-cookies")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())?;
    let f = match std::fs::File::open(cookies_path) {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!("Failed to open cookie file {cookies_path}: {e}");
            return None;
        }
    };
    let jar = std::sync::Arc::new(risuko_http::Jar::new());
    match jar.load_netscape(f) {
        Ok(n) => {
            tracing::info!("Loaded {n} cookies from {cookies_path}");
            Some(jar)
        }
        Err(e) => {
            tracing::warn!("Failed to parse cookie file {cookies_path}: {e}");
            None
        }
    }
}

/// Build custom headers from options
fn build_headers(options: &Map<String, Value>) -> HeaderMap {
    let mut headers = HeaderMap::new();

    if let Some(header_val) = options.get("header").and_then(|v| v.as_str()) {
        for h in header_val.split('\n') {
            let trimmed = h.trim();
            if let Some(colon) = trimmed.find(':') {
                let name = trimmed[..colon].trim();
                let value = trimmed[colon + 1..].trim();
                if let (Ok(n), Ok(v)) = (
                    HeaderName::from_bytes(name.as_bytes()),
                    HeaderValue::from_str(value),
                ) {
                    headers.insert(n, v);
                }
            }
        }
    }

    if let Some(referer) = options
        .get("referer")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        if let Ok(v) = HeaderValue::from_str(referer) {
            headers.insert(risuko_http::header::REFERER, v);
        }
    }

    if let Some(cookie) = options
        .get("cookie")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        if let Ok(v) = HeaderValue::from_str(cookie) {
            headers.insert(risuko_http::header::COOKIE, v);
        }
    }

    headers
}

/// Inject `Authorization: Basic` from a `.netrc` lookup if the URI doesn't
/// already carry credentials and the user hasn't disabled netrc via the
/// `no-netrc` option. Honors `netrc-path` for an override location, and
/// silently no-ops if the file is missing or unreadable. Existing
/// Authorization headers (set via `header` option) are never overwritten
fn apply_netrc_auth(headers: &mut HeaderMap, uri: &str, options: &Map<String, Value>) {
    if headers.contains_key(risuko_http::header::AUTHORIZATION) {
        return;
    }
    if options
        .get("no-netrc")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return;
    }
    let parsed = match url::Url::parse(uri) {
        Ok(u) => u,
        Err(_) => return,
    };
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return;
    }
    let host = match parsed.host_str() {
        Some(h) => h.to_ascii_lowercase(),
        None => return,
    };
    let path = options
        .get("netrc-path")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(super::netrc::default_netrc_path);
    let Some(path) = path else { return };
    let netrc = match super::netrc::Netrc::from_path(&path) {
        Ok(n) => n,
        Err(e) => {
            tracing::debug!("netrc {} unreadable: {e}", path.display());
            return;
        }
    };
    let Some(entry) = netrc.lookup(&host) else {
        return;
    };
    let user = entry.login.as_deref().unwrap_or("");
    let pass = entry.password.as_deref().unwrap_or("");
    if user.is_empty() && pass.is_empty() {
        return;
    }
    let raw = format!("{user}:{pass}");
    use base64::Engine as _;
    let encoded = base64::engine::general_purpose::STANDARD.encode(raw.as_bytes());
    let value = format!("Basic {encoded}");
    if let Ok(v) = HeaderValue::from_str(&value) {
        headers.insert(risuko_http::header::AUTHORIZATION, v);
        tracing::debug!("Applied netrc credentials for {host}");
    }
}

/// Run an HTTP/FTP download. This is the main entry point called from manager.rs
/// Returns the final file path on success
pub async fn run_http_download(
    uri: &str,
    dir: &str,
    out: &str,
    options: &Map<String, Value>,
    total: Arc<AtomicU64>,
    completed: Arc<AtomicU64>,
    speed: Arc<AtomicU64>,
    cancelled: Arc<AtomicBool>,
    connections: Arc<AtomicU32>,
    cancel_token: CancellationToken,
    global_limiter: Arc<SpeedLimiter>,
    task_limiter: Arc<SpeedLimiter>,
    chunk_completed: Vec<Arc<AtomicU64>>,
) -> Result<PathBuf, String> {
    // Single-URI entry point — kept for callers that don't have a mirror
    // list. Internally delegates to the multi-URI path with a 1-element vec
    run_http_download_multi(
        &[uri.to_string()],
        dir,
        out,
        options,
        total,
        completed,
        speed,
        cancelled,
        connections,
        cancel_token,
        global_limiter,
        task_limiter,
        chunk_completed,
    )
    .await
}

/// Multi-URI entry point. Iterates `uris` according to the configured
/// selector strategy. A non-cancellation error from one URI bumps that
/// host's fail count and triggers a try with the next viable URI; the loop
/// terminates on success, on cancellation, or when the selector runs out of
/// candidates
#[allow(clippy::too_many_arguments)]
pub async fn run_http_download_multi(
    uris: &[String],
    dir: &str,
    out: &str,
    options: &Map<String, Value>,
    total: Arc<AtomicU64>,
    completed: Arc<AtomicU64>,
    speed: Arc<AtomicU64>,
    cancelled: Arc<AtomicBool>,
    connections: Arc<AtomicU32>,
    cancel_token: CancellationToken,
    global_limiter: Arc<SpeedLimiter>,
    task_limiter: Arc<SpeedLimiter>,
    chunk_completed: Vec<Arc<AtomicU64>>,
) -> Result<PathBuf, String> {
    if uris.is_empty() {
        return Err("no URIs provided".to_string());
    }
    let strategy = super::uri_selector::strategy_from_options(options);
    let mut stats = super::uri_selector::ServerStats::default();
    let mut tried: Vec<usize> = Vec::new();
    let mut last_err: Option<String> = None;

    loop {
        let idx = match super::uri_selector::pick(strategy, uris, &stats, &tried) {
            Some(i) => i,
            None => {
                return Err(last_err.unwrap_or_else(|| {
                    "all mirrors exhausted with no successful download".to_string()
                }))
            }
        };
        tried.push(idx);
        let uri = &uris[idx];
        if uris.len() > 1 {
            tracing::info!(
                "Mirror {}/{}: {}",
                tried.len(),
                uris.len(),
                super::uri_selector::host_of(uri)
            );
        }

        let started = std::time::Instant::now();
        let result = run_single_uri_download(
            uri,
            dir,
            out,
            options,
            total.clone(),
            completed.clone(),
            speed.clone(),
            cancelled.clone(),
            connections.clone(),
            cancel_token.clone(),
            global_limiter.clone(),
            task_limiter.clone(),
            chunk_completed.clone(),
        )
        .await;

        match result {
            Ok(path) => {
                let elapsed = started.elapsed().as_secs_f64();
                let bytes = completed.load(Ordering::Relaxed);
                stats.record_success(&super::uri_selector::host_of(uri), bytes, elapsed);
                return Ok(path);
            }
            Err(e) => {
                // Cancellation isn't a mirror fault — propagate immediately.
                if cancelled.load(Ordering::Relaxed) || cancel_token.is_cancelled() {
                    return Err(e);
                }
                stats.record_failure(&super::uri_selector::host_of(uri));
                tracing::warn!("Mirror {} failed: {e}", super::uri_selector::host_of(uri));
                last_err = Some(e);
                // Reset counters before retrying the next mirror so progress
                // accounting doesn't double-count partial bytes from a
                // failed attempt.
                completed.store(0, Ordering::Relaxed);
                speed.store(0, Ordering::Relaxed);
                for cc in &chunk_completed {
                    cc.store(0, Ordering::Relaxed);
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_single_uri_download(
    uri: &str,
    dir: &str,
    out: &str,
    options: &Map<String, Value>,
    total: Arc<AtomicU64>,
    completed: Arc<AtomicU64>,
    speed: Arc<AtomicU64>,
    cancelled: Arc<AtomicBool>,
    connections: Arc<AtomicU32>,
    cancel_token: CancellationToken,
    global_limiter: Arc<SpeedLimiter>,
    task_limiter: Arc<SpeedLimiter>,
    chunk_completed: Vec<Arc<AtomicU64>>,
) -> Result<PathBuf, String> {
    tracing::info!("Starting download: uri={uri}, dir={dir}, out={out}");
    let dir_path = Path::new(dir);
    fs::create_dir_all(dir_path).map_err(|e| format!("Failed to create dir: {e}"))?;

    let filename = if out.is_empty() {
        infer_filename_from_uri(uri)
    } else {
        out.to_string()
    };
    // Sanitize: strip path separators and traversal components
    let filename = sanitize_filename(&filename);

    let part_name = if filename.ends_with(PART_SUFFIX) {
        filename.clone()
    } else {
        format!("{filename}{PART_SUFFIX}")
    };
    let part_path = dir_path.join(&part_name);

    let split = options
        .get("split")
        .and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        })
        .unwrap_or(8)
        .max(1) as usize;

    // aria2-compatible `min-split-size` (bytes, with K/M suffixes accepted).
    // Default 1 MiB — smaller files use a single connection.
    let min_split_size = parse_size_option(options.get("min-split-size"))
        .unwrap_or(DEFAULT_MIN_SPLIT_SIZE)
        .max(1);

    // Build a single shared cookie jar so both the decompressing client and
    // the range-request client see the same cookies (load-cookies file is
    // parsed exactly once here)
    let cookie_jar = load_cookie_jar(options);
    let client = build_client(options, cookie_jar.clone())?;
    let range_client = if split > 1 {
        build_client_no_decompress(options, cookie_jar)?
    } else {
        client.clone()
    };
    let mut headers = build_headers(options);
    apply_netrc_auth(&mut headers, uri, options);
    let headers = headers;
    let stall = StallWatchdog::from_options(options);
    let falloc_mode = super::falloc::Mode::from_option(options.get("file-allocation"));

    // Optional integrity checks. Bad input is rejected up-front so a typo
    // doesn't silently disable verification: the user immediately sees the
    // error instead of a "downloaded but unverified" success
    let whole_checksum = parse_whole_checksum_option(options)?;
    let piece_checksums = parse_piece_checksums_option(options)?;

    let use_remote_time = options
        .get("remote-time")
        .and_then(|v| v.as_bool().or_else(|| v.as_str().map(|s| s == "true")))
        .unwrap_or(false);

    // Check for existing partial download
    let existing_size = if part_path.exists() {
        fs::metadata(&part_path).map(|m| m.len()).unwrap_or(0)
    } else {
        0
    };

    let mut last_modified_header: Option<String> = None;

    let is_http = uri.starts_with("http://") || uri.starts_with("https://");
    if is_http && split > 1 {
        // Probe with the no-decompress client so the response headers preserve
        // any `Content-Encoding` the origin advertises. Probing with the
        // decompressing client would strip that header before we could
        // inspect it, defeating the compatibility check
        match probe_range_support(&range_client, uri, &headers).await {
            Ok(Some(probe))
                if probe.content_length > min_split_size.saturating_mul(split as u64) =>
            {
                if use_remote_time {
                    last_modified_header = probe.last_modified.clone();
                }
                connections.store(split as u32, Ordering::Relaxed);
                let result = run_multi_chunk(
                    &range_client,
                    uri,
                    &part_path,
                    probe.content_length,
                    split,
                    &headers,
                    total,
                    completed,
                    speed,
                    cancel_token,
                    &filename,
                    dir_path,
                    global_limiter,
                    task_limiter,
                    probe.etag,
                    &chunk_completed,
                    stall.clone(),
                    falloc_mode,
                )
                .await;
                if let Ok(ref path) = result {
                    if let Some(ref lm) = last_modified_header {
                        apply_remote_file_time(path, lm);
                    }
                    if let Some(ref expected) = piece_checksums {
                        if let Err(e) =
                            verify_piece_checksums(path, probe.content_length, expected).await
                        {
                            tracing::warn!("piece checksum failed, deleting output: {e}");
                            let _ = fs::remove_file(path);
                            return Err(e);
                        }
                    }
                    if let Some(ref expected) = whole_checksum {
                        if let Err(e) = verify_whole_file(path, expected).await {
                            tracing::warn!("integrity check failed, deleting output: {e}");
                            let _ = fs::remove_file(path);
                            return Err(e);
                        }
                    }
                }
                return result;
            }
            Ok(Some(_)) => {
                tracing::info!("File too small for multi-chunk, using single connection");
            }
            Ok(None) => {
                tracing::info!("Server does not support ranges, using single connection");
            }
            Err(e) => {
                tracing::warn!("Range probe failed, falling back to single: {e}");
            }
        }
    }

    // Single-connection download (fallback, resume, FTP, or small files)
    // If falling through from a failed multi-chunk probe and a pre-allocated .part exists,
    // its size doesn't reflect actual download progress — remove it to avoid a 416.
    // Only remove when chunk metadata confirms this is a multi-chunk artifact
    if chunk_meta_path(&part_path).exists() && existing_size > 0 {
        tracing::info!("Removing pre-allocated .part before single-connection fallback");
        let _ = fs::remove_file(&part_path);
        delete_chunk_meta(&part_path);
    }
    connections.store(1, Ordering::Relaxed);
    let result = run_single_download(
        &client,
        uri,
        &part_path,
        &headers,
        total.clone(),
        completed.clone(),
        speed.clone(),
        cancelled.clone(),
        cancel_token.clone(),
        &filename,
        dir_path,
        global_limiter.clone(),
        task_limiter.clone(),
        stall.clone(),
    )
    .await;

    // If a stale .part was removed, retry once from scratch
    let final_result = match result {
        Err(ref e) if e.contains(STALE_PART_REMOVED) => {
            tracing::info!("Retrying download after stale .part removal");
            delete_chunk_meta(&part_path);
            completed.store(0, Ordering::Relaxed);
            total.store(0, Ordering::Relaxed);
            speed.store(0, Ordering::Relaxed);
            for cc in &chunk_completed {
                cc.store(0, Ordering::Relaxed);
            }

            if is_http && split > 1 {
                if let Ok(Some(probe)) = probe_range_support(&range_client, uri, &headers).await {
                    if probe.content_length > min_split_size.saturating_mul(split as u64) {
                        if use_remote_time {
                            last_modified_header = probe.last_modified.clone();
                        }
                        connections.store(split as u32, Ordering::Relaxed);
                        let mc_result = run_multi_chunk(
                            &range_client,
                            uri,
                            &part_path,
                            probe.content_length,
                            split,
                            &headers,
                            total,
                            completed,
                            speed,
                            cancel_token,
                            &filename,
                            dir_path,
                            global_limiter,
                            task_limiter,
                            probe.etag,
                            &chunk_completed,
                            stall.clone(),
                            falloc_mode,
                        )
                        .await;
                        if let Ok(ref path) = mc_result {
                            if let Some(ref lm) = last_modified_header {
                                apply_remote_file_time(path, lm);
                            }
                        }
                        return mc_result;
                    }
                }
            }

            connections.store(1, Ordering::Relaxed);
            run_single_download(
                &client,
                uri,
                &part_path,
                &headers,
                total,
                completed,
                speed,
                cancelled,
                cancel_token,
                &filename,
                dir_path,
                global_limiter,
                task_limiter,
                stall,
            )
            .await
        }
        other => other,
    };

    match final_result {
        Ok((path, lm)) => {
            if use_remote_time {
                if let Some(ref lm_str) = lm {
                    apply_remote_file_time(&path, lm_str);
                }
            }
            // Whole-file integrity: hash the final renamed file. On
            // mismatch, delete it so retry doesn't pick up corrupted bytes
            // as "resumable progress"
            if let Some(ref expected) = whole_checksum {
                if let Err(e) = verify_whole_file(&path, expected).await {
                    tracing::warn!("integrity check failed, deleting output: {e}");
                    let _ = fs::remove_file(&path);
                    return Err(e);
                }
            }
            Ok(path)
        }
        Err(e) => Err(e),
    }
}

/// Result from probing range support: content length + optional ETag
struct ProbeResult {
    content_length: u64,
    etag: Option<String>,
    last_modified: Option<String>,
}

/// Probe whether the server supports Range requests
/// Returns Some(ProbeResult) if ranges are supported, None otherwise
async fn probe_range_support(
    client: &Client,
    uri: &str,
    headers: &HeaderMap,
) -> Result<Option<ProbeResult>, String> {
    let resp = client
        .get(uri)
        .headers(headers.clone())
        .header(RANGE, "bytes=0-0")
        .header(ACCEPT_ENCODING, "identity")
        .send()
        .await
        .map_err(|e| format!("Range probe request failed: {e}"))?;

    let status = resp.status().as_u16();
    let etag = resp
        .headers()
        .get(ETAG)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let last_modified = resp
        .headers()
        .get(LAST_MODIFIED)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // Compatibility short-circuits: if the server returns a compressed body
    // or chunked transfer-encoding (no length), parallel range workers can't
    // safely slice the file. Range offsets refer to the *encoded* bytes when
    // Content-Encoding is present, but the worker sees decoded bytes — the
    // two address spaces are incompatible. The chunked path additionally
    // means we don't know the total size up front. Fall back to the single
    // streaming path in either case
    let content_encoding = resp
        .headers()
        .get(CONTENT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !content_encoding.is_empty() && !content_encoding.eq_ignore_ascii_case("identity") {
        tracing::info!(
            "Range probe: Content-Encoding={content_encoding}; falling back to single connection"
        );
        return Ok(None);
    }
    let transfer_encoding = resp
        .headers()
        .get(TRANSFER_ENCODING)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if transfer_encoding
        .split(',')
        .any(|t| t.trim().eq_ignore_ascii_case("chunked"))
    {
        tracing::info!("Range probe: Transfer-Encoding=chunked; falling back to single connection");
        return Ok(None);
    }

    if status == 206 {
        // Parse Content-Range: bytes 0-0/TOTAL
        if let Some(cr) = resp
            .headers()
            .get(CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
        {
            if let Some(slash) = cr.rfind('/') {
                if let Ok(total) = cr[slash + 1..].trim().parse::<u64>() {
                    return Ok(Some(ProbeResult {
                        content_length: total,
                        etag,
                        last_modified,
                    }));
                }
            }
        }
        return Ok(None);
    }

    if status == 200 {
        // Server ignored Range header, check Accept-Ranges
        if let Some(ar) = resp
            .headers()
            .get(ACCEPT_RANGES)
            .and_then(|v| v.to_str().ok())
        {
            if ar.eq_ignore_ascii_case("bytes") {
                if let Some(cl) = resp
                    .headers()
                    .get(CONTENT_LENGTH)
                    .and_then(|v| v.to_str().ok())
                {
                    if let Ok(total) = cl.trim().parse::<u64>() {
                        if total > 0 {
                            return Ok(Some(ProbeResult {
                                content_length: total,
                                etag,
                                last_modified,
                            }));
                        }
                    }
                }
            }
        }
        return Ok(None);
    }

    Ok(None)
}

/// Multi-chunk parallel download using a piece queue + worker pool.
///
/// The file is divided into PIECE_SIZE-byte pieces. `split` workers share
/// one queue: each pulls the next free piece, downloads it, then pulls the
/// next. Fast workers naturally pull more pieces (work stealing for free).
/// Resume preserves per-piece byte progress so a SIGKILL never loses more
/// than the in-flight bytes of one piece per worker
async fn run_multi_chunk(
    client: &Client,
    uri: &str,
    part_path: &Path,
    content_length: u64,
    split: usize,
    headers: &HeaderMap,
    total: Arc<AtomicU64>,
    completed: Arc<AtomicU64>,
    speed: Arc<AtomicU64>,
    cancel_token: CancellationToken,
    filename: &str,
    dir_path: &Path,
    global_limiter: Arc<SpeedLimiter>,
    task_limiter: Arc<SpeedLimiter>,
    expected_etag: Option<String>,
    chunk_completed: &[Arc<AtomicU64>],
    stall: StallWatchdog,
    falloc_mode: super::falloc::Mode,
) -> Result<PathBuf, String> {
    total.store(content_length, Ordering::Relaxed);

    let queue = Arc::new(PieceQueue::new(content_length));
    tracing::info!(
        "Multi-piece download: {} pieces ({} MiB each), {} workers, {} bytes total",
        queue.pieces.len(),
        PIECE_SIZE / (1024 * 1024),
        split,
        content_length
    );

    // Restore piece progress from sidecar BEFORE pre-allocating so the
    // file-size check inside load_piece_meta is meaningful.
    if let Some(meta) = load_piece_meta(part_path, content_length, &expected_etag) {
        let mut total_resumed: u64 = 0;
        for pp in &meta.pieces {
            if let Some(p) = queue.pieces.get(pp.i as usize) {
                let c = std::cmp::min(pp.c, p.length);
                p.completed.store(c, Ordering::Relaxed);
                total_resumed += c as u64;
                if c == p.length {
                    p.state.store(PIECE_DONE, Ordering::Release);
                }
            }
        }
        completed.store(total_resumed, Ordering::Relaxed);
        tracing::info!("Resuming multi-piece download: {total_resumed}/{content_length} bytes");
    }

    // Pre-allocate and open the output file as a single shared std::fs::File.
    // All writers issue positioned writes (pwrite/seek_write) concurrently —
    // no global mutex, no shared file cursor.
    //
    // The allocation strategy is configurable via `file-allocation`
    // (`falloc` | `trunc` | `none`). `falloc` reserves real disk blocks via
    // platform-specific syscalls; on filesystems that don't support it we
    // silently degrade to `trunc` (`set_len`)
    let file = {
        let f = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(part_path)
            .map_err(|e| format!("Failed to create file: {e}"))?;
        super::falloc::allocate(&f, content_length, falloc_mode)
            .map_err(|e| format!("Failed to pre-allocate file: {e}"))?;
        Arc::new(f)
    };

    // Speed tracker
    let speed_cancel = cancel_token.clone();
    let speed_completed = completed.clone();
    let speed_val = speed.clone();
    let speed_total = total.clone();
    let speed_stall = stall.clone();
    let speed_task = tokio::spawn(async move {
        run_speed_tracker(
            speed_completed,
            speed_val,
            speed_total,
            speed_cancel,
            speed_stall,
        )
        .await;
    });

    // Periodic sidecar save: snapshot piece progress every META_SAVE_INTERVAL
    // so a SIGKILL never loses more than that interval of in-flight bytes.
    let save_part = part_path.to_path_buf();
    let save_queue = Arc::clone(&queue);
    let save_etag = expected_etag.clone();
    let save_cancel = cancel_token.clone();
    let save_task = tokio::spawn(async move {
        let mut tick = tokio::time::interval(META_SAVE_INTERVAL);
        tick.tick().await; // skip the immediate first tick
        loop {
            tokio::select! {
                _ = save_cancel.cancelled() => break,
                _ = tick.tick() => {
                    save_piece_meta(&save_part, &save_queue, content_length, &save_etag);
                }
            }
        }
    });

    // Spawn N=split worker tasks. Each worker pulls pieces off the shared
    // queue until it is exhausted (or the task is cancelled / fails).
    let mut workers = futures_util::stream::FuturesUnordered::new();
    for w in 0..split {
        let client = client.clone();
        let uri = uri.to_string();
        let headers = headers.clone();
        let file = Arc::clone(&file);
        let queue = Arc::clone(&queue);
        let completed = Arc::clone(&completed);
        let cancel_token = cancel_token.clone();
        let gl = Arc::clone(&global_limiter);
        let tl = Arc::clone(&task_limiter);
        let etag = expected_etag.clone();
        let wc = chunk_completed.get(w).cloned();

        workers.push(tokio::spawn(async move {
            piece_worker(
                w,
                &client,
                &uri,
                &headers,
                file,
                queue,
                completed,
                cancel_token,
                gl,
                tl,
                etag,
                wc,
            )
            .await
        }));
    }

    let mut errors = Vec::new();
    while let Some(result) = workers.next().await {
        match result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => errors.push(e),
            Err(e) => errors.push(format!("Worker task panicked: {e}")),
        }
    }

    speed_task.abort();
    save_task.abort();
    // Drain the save task: abort cancels at the next await, but a save
    // iteration mid-flight can still complete its synchronous fs::write.
    // Awaiting here guarantees no save lands on disk after we proceed to
    // delete_chunk_meta below.
    let _ = save_task.await;
    speed.store(0, Ordering::Relaxed);

    // Stall watchdog tripped — surface a distinct, retryable error rather
    // than letting the generic "cancelled" classification swallow it. The
    // sidecar still holds piece progress so a retry resumes in place
    if stall.flag.load(Ordering::Acquire) {
        save_piece_meta(part_path, &queue, content_length, &expected_etag);
        return Err(format!(
            "Download stalled: speed below {} B/s for {}s",
            stall.lowest_speed,
            stall.timeout.as_secs(),
        ));
    }

    if !errors.is_empty() {
        // Persist current piece progress so the next attempt resumes correctly.
        save_piece_meta(part_path, &queue, content_length, &expected_etag);

        if errors.iter().all(|e| e.contains("cancelled")) {
            return Err("Download cancelled".to_string());
        }
        // Worker errors are only fatal if the queue itself did not finish.
        // A worker that exhausted its retry budget on one piece may have
        // returned Err while another worker later picked the piece up and
        // completed it. In that case the download is actually done
        if !queue.is_finished() {
            let real_errors: Vec<&String> =
                errors.iter().filter(|e| !e.contains("cancelled")).collect();
            if !real_errors.is_empty() {
                return Err(real_errors
                    .iter()
                    .map(|e| e.as_str())
                    .collect::<Vec<_>>()
                    .join("; "));
            }
        }
    }

    // Ensure all positioned writes are durable before deleting the resume
    // sidecar or renaming. If sync_file fails (ENOSPC, EIO, …) the .part
    // file may be incomplete on disk — abort without finalizing so the
    // sidecar survives and the next attempt can resume
    if let Err(e) = sync_file(&file).await {
        return Err(format!("fsync before rename failed: {e}"));
    }
    delete_chunk_meta(part_path);
    finalize_download(part_path, filename, dir_path)
}

/// Worker loop: claim a piece, download it, mark it done; repeat.
/// Returns Ok(()) when the queue is exhausted (this worker is finished),
/// or Err on cancellation or after exceeding retry budget on one piece.
#[allow(clippy::too_many_arguments)]
async fn piece_worker(
    worker_id: usize,
    client: &Client,
    uri: &str,
    headers: &HeaderMap,
    file: Arc<std::fs::File>,
    queue: Arc<PieceQueue>,
    completed: Arc<AtomicU64>,
    cancel_token: CancellationToken,
    global_limiter: Arc<SpeedLimiter>,
    task_limiter: Arc<SpeedLimiter>,
    expected_etag: Option<String>,
    worker_completed: Option<Arc<AtomicU64>>,
) -> Result<(), String> {
    let mut retry_count: u32 = 0;
    loop {
        if cancel_token.is_cancelled() {
            return Err("Download cancelled".to_string());
        }

        let idx = match queue.claim_next() {
            Some(i) => i,
            None => return Ok(()), // queue exhausted: this worker is done
        };

        let piece_offset = queue.pieces[idx].offset;
        let piece_length = queue.pieces[idx].length;
        let piece_completed = Arc::clone(&queue.pieces[idx].completed);

        let already = piece_completed.load(Ordering::Relaxed);
        if already >= piece_length {
            queue.complete(idx);
            continue;
        }

        let range = ChunkRange::new(
            piece_offset + already as u64,
            piece_offset + piece_length as u64 - 1,
        );

        let outcome = download_piece_stream(
            client,
            uri,
            headers,
            range,
            &file,
            &completed,
            &cancel_token,
            &global_limiter,
            &task_limiter,
            expected_etag.as_deref(),
            worker_completed.as_ref(),
            &piece_completed,
        )
        .await;

        // The writer task already incremented piece_completed per Bytes flushed.
        let now_completed = piece_completed.load(Ordering::Relaxed);

        match outcome.error {
            None => {
                if now_completed >= piece_length {
                    queue.complete(idx);
                    retry_count = 0;
                } else {
                    // Server closed the stream early — return the piece to the
                    // pool with its progress preserved; another worker (or
                    // this one on a later iteration) will pick up the tail.
                    // Count the early EOF against the retry budget so a
                    // server that consistently truncates responses cannot
                    // pin a worker in an infinite loop
                    queue.release(idx);
                    retry_count += 1;
                    if retry_count > CHUNK_MAX_RETRIES {
                        return Err(format!(
                            "Worker {worker_id} failed after {CHUNK_MAX_RETRIES} retries on \
                             piece {idx}: server closed stream early"
                        ));
                    }
                    tracing::warn!(
                        "Worker {worker_id} piece {idx} attempt \
                         {retry_count}/{CHUNK_MAX_RETRIES}: early EOF, will retry"
                    );
                    tokio::time::sleep(std::time::Duration::from_secs(retry_count as u64)).await;
                }
            }
            Some(e) if e.contains("cancelled") => {
                queue.release(idx);
                return Err(e);
            }
            Some(e) => {
                queue.release(idx);
                retry_count += 1;
                if retry_count > CHUNK_MAX_RETRIES {
                    return Err(format!(
                        "Worker {worker_id} failed after {CHUNK_MAX_RETRIES} retries on \
                         piece {idx}: {e}"
                    ));
                }
                tracing::warn!(
                    "Worker {worker_id} piece {idx} attempt \
                     {retry_count}/{CHUNK_MAX_RETRIES}: {e}, will retry"
                );
                tokio::time::sleep(std::time::Duration::from_secs(retry_count as u64)).await;
            }
        }
    }
}

/// Stream a single piece (or its remaining tail). Always returns the number
/// of bytes flushed to disk, even on error, so the caller can accurately
/// track resume progress via the per-piece atomic counter.
#[allow(clippy::too_many_arguments)]
async fn download_piece_stream(
    client: &Client,
    uri: &str,
    headers: &HeaderMap,
    range: ChunkRange,
    file: &Arc<std::fs::File>,
    completed: &Arc<AtomicU64>,
    cancel_token: &CancellationToken,
    global_limiter: &SpeedLimiter,
    task_limiter: &SpeedLimiter,
    expected_etag: Option<&str>,
    worker_completed: Option<&Arc<AtomicU64>>,
    piece_completed: &Arc<AtomicU32>,
) -> StreamOutcome {
    let mut req = client
        .get(uri)
        .headers(headers.clone())
        .header(RANGE, range.to_range_header_value());

    if let Some(etag) = expected_etag {
        if let Ok(v) = HeaderValue::from_str(etag) {
            req = req.header(IF_MATCH, v);
        }
    }

    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            return StreamOutcome {
                error: Some(format!("HTTP request failed: {e}")),
            };
        }
    };

    let status = resp.status().as_u16();

    if let Some(expected) = expected_etag {
        if let Some(actual) = resp.headers().get(ETAG).and_then(|v| v.to_str().ok()) {
            if actual != expected {
                return StreamOutcome {
                    error: Some(
                        "Server file changed (ETag mismatch), aborting download".to_string(),
                    ),
                };
            }
        }
    }

    if status >= 400 {
        return StreamOutcome {
            error: Some(format!("HTTP error: {status}")),
        };
    }

    if status != 206 {
        return StreamOutcome {
            error: Some(format!(
                "Expected 206 Partial Content with matching Content-Range, got {status}"
            )),
        };
    }
    if let Some(cr) = resp
        .headers()
        .get(CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
    {
        if let Some(space) = cr.find(' ') {
            if let Some(dash) = cr[space + 1..].find('-') {
                if let Ok(range_start) = cr[space + 1..space + 1 + dash].parse::<u64>() {
                    if range_start != range.start {
                        return StreamOutcome {
                            error: Some(format!(
                                "Expected 206 Partial Content with matching Content-Range: \
                                 requested start {} but got {range_start}",
                                range.start
                            )),
                        };
                    }
                }
            }
        }
    } else {
        return StreamOutcome {
            error: Some(
                "Expected 206 Partial Content with matching Content-Range, \
                 but Content-Range header is missing"
                    .to_string(),
            ),
        };
    }

    let mut stream = resp.bytes_stream();
    // Cap writes at the requested range length. A misbehaving server that
    // returns more bytes than asked must NOT pwrite past the piece into the
    // next piece's region in the pre-allocated file.
    let max_bytes = range.end - range.start + 1;
    let writer = ChunkWriter::spawn(
        Arc::clone(file),
        range.start,
        Some(max_bytes),
        Arc::clone(completed),
        worker_completed.cloned(),
        Some(Arc::clone(piece_completed)),
    );

    loop {
        tokio::select! {
            _ = cancel_token.cancelled() => {
                let _ = writer.finish().await;
                return StreamOutcome {
                    error: Some("Download cancelled".to_string()),
                };
            }
            chunk = stream.next() => {
                match chunk {
                    Some(Ok(bytes)) => {
                        let len = bytes.len();
                        global_limiter.acquire(len).await;
                        task_limiter.acquire(len).await;

                        if !writer.send(bytes).await {
                            let _ = writer.finish().await;
                            return StreamOutcome {
                                error: Some(
                                    "Writer task closed unexpectedly".to_string(),
                                ),
                            };
                        }
                    }
                    Some(Err(e)) => {
                        let _ = writer.finish().await;
                        return StreamOutcome {
                            error: Some(format!("Stream error: {e}")),
                        };
                    }
                    None => {
                        match writer.finish().await {
                            Ok(_) => {
                                return StreamOutcome { error: None };
                            }
                            Err(e) => {
                                return StreamOutcome {
                                    error: Some(e),
                                };
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Parse a JSON value as a byte-size: integer bytes, or a string with optional
/// `K`/`M`/`G` suffix (case-insensitive). Returns `None` for missing/invalid.
fn parse_size_option(value: Option<&Value>) -> Option<u64> {
    let v = value?;
    if let Some(n) = v.as_u64() {
        return Some(n);
    }
    let s = v.as_str()?.trim();
    if s.is_empty() {
        return None;
    }
    let (num, mult): (&str, u64) = match s.as_bytes().last() {
        Some(b'K') | Some(b'k') => (&s[..s.len() - 1], 1024),
        Some(b'M') | Some(b'm') => (&s[..s.len() - 1], 1024 * 1024),
        Some(b'G') | Some(b'g') => (&s[..s.len() - 1], 1024 * 1024 * 1024),
        _ => (s, 1),
    };
    num.trim().parse::<u64>().ok()?.checked_mul(mult)
}

/// Cross-platform positioned write: writes the entire buffer at the given
/// offset without touching the shared file cursor. Safe to call concurrently
/// from multiple threads on the same `File` handle.
#[cfg(unix)]
fn pwrite_all(file: &std::fs::File, offset: u64, buf: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::FileExt;
    file.write_all_at(buf, offset)
}

#[cfg(windows)]
fn pwrite_all(file: &std::fs::File, offset: u64, buf: &[u8]) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    let mut written = 0usize;
    while written < buf.len() {
        let n = file.seek_write(&buf[written..], offset + written as u64)?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "failed to write whole buffer",
            ));
        }
        written += n;
    }
    Ok(())
}

/// fsync via blocking thread.
async fn sync_file(file: &Arc<std::fs::File>) -> Result<(), String> {
    let file = Arc::clone(file);
    tokio::task::spawn_blocking(move || file.sync_all())
        .await
        .map_err(|e| format!("sync task join error: {e}"))?
        .map_err(|e| format!("Sync failed: {e}"))
}

/// Dedicated chunk writer: a single `spawn_blocking` thread per piece that
/// pulls `Bytes` from an MPSC channel and `pwrite`s them sequentially to the
/// pre-allocated file. No userspace memcpy (Bytes flows straight from HTTP
/// client to pwrite), no per-flush spawn_blocking churn.
struct ChunkWriter {
    tx: tokio::sync::mpsc::Sender<Bytes>,
    join: tokio::task::JoinHandle<Result<u64, String>>,
}

impl ChunkWriter {
    /// `max_bytes` caps the total bytes this writer will pwrite. Excess input
    /// is silently dropped so a misbehaving server returning more data than
    /// the requested Range can never overwrite adjacent pieces in the
    /// pre-allocated file. `None` means unlimited (single-connection path).
    fn spawn(
        file: Arc<std::fs::File>,
        start_offset: u64,
        max_bytes: Option<u64>,
        completed: Arc<AtomicU64>,
        worker_completed: Option<Arc<AtomicU64>>,
        piece_completed: Option<Arc<AtomicU32>>,
    ) -> Self {
        // Bounded channel = backpressure. Cap ~4 MiB worth of in-flight Bytes
        // (16 messages * typical 16-256 KiB each) so we never balloon RAM if
        // the disk is briefly slower than the network.
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Bytes>(16);
        let join = tokio::task::spawn_blocking(move || {
            let mut offset = start_offset;
            let mut written: u64 = 0;
            let mut remaining = max_bytes;
            while let Some(mut bytes) = rx.blocking_recv() {
                if let Some(rem) = remaining {
                    if rem == 0 {
                        // Drain extra data without writing — server overran.
                        continue;
                    }
                    if (bytes.len() as u64) > rem {
                        bytes.truncate(rem as usize);
                    }
                }
                if bytes.is_empty() {
                    continue;
                }
                if let Err(e) = pwrite_all(&file, offset, &bytes) {
                    return Err(format!("Write failed: {e}"));
                }
                let n = bytes.len() as u64;
                offset += n;
                written += n;
                if let Some(rem) = remaining.as_mut() {
                    *rem -= n;
                }
                completed.fetch_add(n, Ordering::Relaxed);
                if let Some(ref wc) = worker_completed {
                    wc.fetch_add(n, Ordering::Relaxed);
                }
                if let Some(ref pc) = piece_completed {
                    pc.fetch_add(n as u32, Ordering::Relaxed);
                }
            }
            Ok(written)
        });
        Self { tx, join }
    }

    /// Send a Bytes to the writer. Returns false if the writer has died.
    async fn send(&self, bytes: Bytes) -> bool {
        self.tx.send(bytes).await.is_ok()
    }

    /// Close the input side and await the writer thread. Returns total bytes
    /// successfully written or the writer's first error.
    async fn finish(self) -> Result<u64, String> {
        drop(self.tx);
        match self.join.await {
            Ok(r) => r,
            Err(e) => Err(format!("write task join error: {e}")),
        }
    }
}

/// Single-connection download
/// Returns (final_path, last_modified_header_value)
async fn run_single_download(
    client: &Client,
    uri: &str,
    part_path: &Path,
    headers: &HeaderMap,
    total: Arc<AtomicU64>,
    completed: Arc<AtomicU64>,
    speed: Arc<AtomicU64>,
    cancelled: Arc<AtomicBool>,
    cancel_token: CancellationToken,
    filename: &str,
    dir_path: &Path,
    global_limiter: Arc<SpeedLimiter>,
    task_limiter: Arc<SpeedLimiter>,
    stall: StallWatchdog,
) -> Result<(PathBuf, Option<String>), String> {
    let existing_size = if part_path.exists() {
        fs::metadata(part_path).map(|m| m.len()).unwrap_or(0)
    } else {
        0
    };

    completed.store(existing_size, Ordering::Relaxed);

    let mut req = client.get(uri).headers(headers.clone());
    if existing_size > 0 {
        req = req.header(RANGE, format!("bytes={existing_size}-"));
    }

    let resp = req.send().await.map_err(|e| {
        if cancelled.load(Ordering::Relaxed) {
            "Download cancelled".to_string()
        } else {
            format!("Download failed: {e}")
        }
    })?;

    let status = resp.status().as_u16();

    if status == 416 && existing_size > 0 {
        tracing::warn!("Got 416 with existing_size={existing_size}, deleting stale .part");
        let _ = fs::remove_file(part_path);
        return Err(format!("Download will retry: {STALE_PART_REMOVED}"));
    }

    if status >= 400 {
        return Err(format!("HTTP error: {status}"));
    }

    let resp_last_modified = resp
        .headers()
        .get(LAST_MODIFIED)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // Update total from Content-Length
    if let Some(cl) = resp.content_length() {
        if cl > 0 {
            total.store(existing_size + cl, Ordering::Relaxed);
        }
    }

    // Open file as a sync handle for positioned writes. We start writing at
    // `existing_size` to resume in place \u2014 no append mode, no shared cursor.
    let file = {
        let f = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(part_path)
            .map_err(|e| format!("Failed to open file: {e}"))?;
        Arc::new(f)
    };

    // Speed tracking
    let speed_cancel = cancel_token.clone();
    let speed_completed = completed.clone();
    let speed_val = speed.clone();
    let speed_total = total.clone();
    let speed_stall = stall.clone();
    let speed_task = tokio::spawn(async move {
        run_speed_tracker(
            speed_completed,
            speed_val,
            speed_total,
            speed_cancel,
            speed_stall,
        )
        .await;
    });

    let mut stream = resp.bytes_stream();
    // Single-connection path also uses the dedicated writer thread:
    // zero-copy Bytes -> pwrite, no userspace memcpy. No max_bytes cap here
    // — the file isn't pre-allocated, so trailing extra bytes are appended
    // rather than corrupting other regions.
    let writer = ChunkWriter::spawn(
        Arc::clone(&file),
        existing_size,
        None,
        Arc::clone(&completed),
        None,
        None,
    );

    let result: Result<(), String> = loop {
        tokio::select! {
            _ = cancel_token.cancelled() => {
                break Err("Download cancelled".to_string());
            }
            chunk = stream.next() => {
                match chunk {
                    Some(Ok(bytes)) => {
                        let len = bytes.len();
                        global_limiter.acquire(len).await;
                        task_limiter.acquire(len).await;
                        if !writer.send(bytes).await {
                            break Err("Writer task closed unexpectedly".to_string());
                        }
                    }
                    Some(Err(e)) => {
                        break Err(format!("Download failed: {e}"));
                    }
                    None => {
                        break Ok(());
                    }
                }
            }
        }
    };

    // Always drain the writer so all queued Bytes hit disk before we sync.
    let writer_result = writer.finish().await;

    let result = match (result, writer_result) {
        (Ok(()), Ok(_)) => sync_file(&file).await,
        (Ok(()), Err(e)) => Err(e),
        (Err(e), _) => Err(e),
    };

    speed_task.abort();
    speed.store(0, Ordering::Relaxed);

    if stall.flag.load(Ordering::Acquire) {
        return Err(format!(
            "Download stalled: speed below {} B/s for {}s",
            stall.lowest_speed,
            stall.timeout.as_secs(),
        ));
    }

    result?;
    let final_path = finalize_download(part_path, filename, dir_path)?;
    Ok((final_path, resp_last_modified))
}

/// Speed tracker that samples completed bytes every 250ms using EMA. Also
/// implements aria2's `--lowest-speed-limit` watchdog: when the EMA stays
/// below `stall.lowest_speed` (bytes/s) for at least `stall.timeout` seconds,
/// `stall.flag` is raised and `cancel_token` is cancelled so workers exit
/// Both the multi-piece and single-connection paths inspect `stall.flag`
/// after their workers complete to surface the stall as a distinct error
async fn run_speed_tracker(
    completed: Arc<AtomicU64>,
    speed: Arc<AtomicU64>,
    total: Arc<AtomicU64>,
    cancel_token: CancellationToken,
    stall: StallWatchdog,
) {
    let started_at = tokio::time::Instant::now();
    let mut last_bytes = completed.load(Ordering::Relaxed);
    let mut last_time = started_at;
    let mut ema_speed: f64 = 0.0;
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(250));

    // Watchdog state. `below_since` records when the EMA first dropped below
    // the threshold. Once `total` is known we arm immediately so the user-
    // configured budget kicks in. For chunked / unknown-length responses
    // (`total == 0`) we wait `unknown_len_grace` after start before arming so
    // that pre-flight time (setup, probe, first byte) doesn't count against
    // the budget but truly stalled streams still get killed
    let mut below_since: Option<tokio::time::Instant> = None;
    let watchdog_active = stall.lowest_speed > 0;
    let unknown_len_grace = stall.timeout;

    loop {
        interval.tick().await;
        if cancel_token.is_cancelled() {
            break;
        }

        let now = tokio::time::Instant::now();
        let elapsed = now.duration_since(last_time).as_secs_f64();
        let current = completed.load(Ordering::Relaxed);

        if elapsed > 0.0 {
            let delta = current.saturating_sub(last_bytes);
            let instant_speed = delta as f64 / elapsed;

            if ema_speed < 1.0 {
                ema_speed = instant_speed;
            } else {
                ema_speed = SPEED_EMA_ALPHA * instant_speed + (1.0 - SPEED_EMA_ALPHA) * ema_speed;
            }
            speed.store(ema_speed as u64, Ordering::Relaxed);
            last_bytes = current;
            last_time = now;
        }

        let t = total.load(Ordering::Relaxed);
        let armed =
            watchdog_active && (t > 0 || now.duration_since(started_at) >= unknown_len_grace);
        if armed {
            if (ema_speed as u64) < stall.lowest_speed {
                let started = below_since.get_or_insert(now);
                if now.duration_since(*started) >= stall.timeout {
                    tracing::warn!(
                        "Download stalled: EMA {:.0} B/s below {} B/s for {}s",
                        ema_speed,
                        stall.lowest_speed,
                        stall.timeout.as_secs(),
                    );
                    stall.flag.store(true, Ordering::Release);
                    cancel_token.cancel();
                    break;
                }
            } else {
                below_since = None;
            }
        }

        if t > 0 && current >= t {
            break;
        }
    }
    speed.store(0, Ordering::Relaxed);
}

/// Rename the .part file to the final filename
fn finalize_download(part_path: &Path, filename: &str, dir_path: &Path) -> Result<PathBuf, String> {
    let final_name = if let Some(stripped) = filename.strip_suffix(PART_SUFFIX) {
        stripped.to_string()
    } else {
        filename.to_string()
    };
    let final_path = dir_path.join(&final_name);
    if part_path != final_path {
        fs::rename(part_path, &final_path).map_err(|e| format!("Failed to rename: {e}"))?;
    }
    Ok(final_path)
}

/// Apply the remote server's Last-Modified time to the downloaded file.
/// `last_modified_str` is the raw HTTP Last-Modified header value (RFC 2822 / RFC 7231).
fn apply_remote_file_time(path: &Path, last_modified_str: &str) {
    if let Some(ft) = parse_http_date(last_modified_str) {
        if let Err(e) = filetime::set_file_mtime(path, ft) {
            tracing::warn!("Failed to set remote file time on {}: {e}", path.display());
        } else {
            tracing::info!(
                "Set remote file time on {}: {last_modified_str}",
                path.display()
            );
        }
    } else {
        tracing::warn!("Could not parse Last-Modified header: {last_modified_str}");
    }
}

/// Parse an HTTP date string into a `FileTime`.
/// Supports the three formats allowed by RFC 7231:
///   - `Sun, 06 Nov 1994 08:49:37 GMT` (preferred)
///   - `Sunday, 06-Nov-94 08:49:37 GMT`
///   - `Sun Nov  6 08:49:37 1994`
fn parse_http_date(s: &str) -> Option<filetime::FileTime> {
    // Try std::time parsing via httpdate-style manual parse
    let s = s.trim();
    // Use the simple approach: try to parse common HTTP date formats
    static MONTHS: &[&str] = &[
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];

    fn month_num(m: &str) -> Option<u32> {
        MONTHS
            .iter()
            .position(|&name| name.eq_ignore_ascii_case(m))
            .map(|i| i as u32 + 1)
    }

    // Try RFC 7231 preferred format: "Sun, 06 Nov 1994 08:49:37 GMT"
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() == 6 && parts[5].eq_ignore_ascii_case("GMT") {
        let day: u32 = parts[1].trim_end_matches(',').parse().ok()?;
        let month = month_num(parts[2])?;
        let year: i64 = parts[3].parse().ok()?;
        let time_parts: Vec<&str> = parts[4].split(':').collect();
        if time_parts.len() == 3 {
            let hour: u32 = time_parts[0].parse().ok()?;
            let min: u32 = time_parts[1].parse().ok()?;
            let sec: u32 = time_parts[2].parse().ok()?;

            // Calculate seconds since epoch
            let days = days_from_civil(year, month, day)?;
            let secs = days * 86400 + hour as i64 * 3600 + min as i64 * 60 + sec as i64;
            if secs >= 0 {
                return Some(filetime::FileTime::from_unix_time(secs, 0));
            }
        }
    }

    None
}

/// Convert a civil date (year, month, day) to days since Unix epoch.
/// Algorithm from Howard Hinnant.
fn days_from_civil(y: i64, m: u32, d: u32) -> Option<i64> {
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let y = if m <= 2 { y - 1 } else { y };
    let m = m as i64;
    let d = d as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe as i64 * 365 + yoe as i64 / 4 - yoe as i64 / 100 + doy;
    Some(era * 146097 + doe - 719468)
}

/// Sanitize a filename to prevent path traversal attacks.
/// Strips directory components, `..`, and path separators.
fn sanitize_filename(name: &str) -> String {
    let base = Path::new(name)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "download".to_string());
    if base.is_empty() || base == "." || base == ".." {
        "download".to_string()
    } else {
        base
    }
}

pub fn infer_filename_from_uri(uri: &str) -> String {
    let without_hash = uri.split('#').next().unwrap_or(uri);
    let without_query = without_hash.split('?').next().unwrap_or(without_hash);
    let candidate = without_query.rsplit('/').next().unwrap_or("").trim();
    if candidate.is_empty() || !candidate.contains('.') {
        "download".to_string()
    } else {
        url_decode(candidate)
    }
}

fn url_decode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                result.push((hi << 4 | lo) as char);
                i += 3;
                continue;
            }
        }
        result.push(bytes[i] as char);
        i += 1;
    }
    result
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn piece_queue_partitions_content_into_1mib_pieces() {
        let total = PIECE_SIZE * 3 + 7;
        let q = PieceQueue::new(total);
        assert_eq!(q.pieces.len(), 4);
        assert_eq!(q.pieces[0].offset, 0);
        assert_eq!(q.pieces[0].length, PIECE_SIZE as u32);
        assert_eq!(q.pieces[3].offset, PIECE_SIZE * 3);
        assert_eq!(q.pieces[3].length, 7);
        let sum: u64 = q.pieces.iter().map(|p| p.length as u64).sum();
        assert_eq!(sum, total);
    }

    #[test]
    fn claim_marks_inflight_and_skips_busy_pieces() {
        let q = PieceQueue::new(PIECE_SIZE * 3);
        let a = q.claim_next().unwrap();
        let b = q.claim_next().unwrap();
        let c = q.claim_next().unwrap();
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert!(q.claim_next().is_none(), "queue should be exhausted");
    }

    #[test]
    fn release_returns_piece_to_pool_for_work_stealing() {
        let q = PieceQueue::new(PIECE_SIZE * 2);
        let a = q.claim_next().unwrap();
        let b = q.claim_next().unwrap();
        assert!(q.claim_next().is_none());
        q.release(a);
        // Another worker can pick up the released piece (the essence of stealing)
        let stolen = q.claim_next().unwrap();
        assert_eq!(stolen, a);
        // b is still in flight
        q.complete(b);
        q.complete(a);
        assert!(q.claim_next().is_none());
    }

    #[test]
    fn complete_prevents_reclaim() {
        let q = PieceQueue::new(PIECE_SIZE * 2);
        let a = q.claim_next().unwrap();
        q.complete(a);
        let b = q.claim_next().unwrap();
        assert_ne!(b, a, "completed piece must not be reclaimed");
        q.complete(b);
        assert!(q.claim_next().is_none());
    }

    #[test]
    fn piece_meta_round_trip_is_sparse() {
        let dir = std::env::temp_dir().join(format!("risuko_piecemeta_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let part = dir.join("file.bin.part");
        std::fs::write(&part, vec![0u8; (PIECE_SIZE * 3) as usize]).unwrap();

        let q = Arc::new(PieceQueue::new(PIECE_SIZE * 3));
        // Mark piece 0 fully done, piece 1 half done, piece 2 untouched.
        q.pieces[0]
            .completed
            .store(PIECE_SIZE as u32, Ordering::Relaxed);
        q.pieces[0].state.store(PIECE_DONE, Ordering::Release);
        q.pieces[1]
            .completed
            .store(PIECE_SIZE as u32 / 2, Ordering::Relaxed);

        let etag = Some("\"abc\"".to_string());
        save_piece_meta(&part, &q, PIECE_SIZE * 3, &etag);

        let loaded = load_piece_meta(&part, PIECE_SIZE * 3, &etag).expect("meta");
        assert_eq!(loaded.version, META_VERSION);
        assert_eq!(loaded.content_length, PIECE_SIZE * 3);
        // Sparse: only pieces with c > 0 are recorded.
        assert_eq!(loaded.pieces.len(), 2);
        let p0 = loaded.pieces.iter().find(|p| p.i == 0).unwrap();
        let p1 = loaded.pieces.iter().find(|p| p.i == 1).unwrap();
        assert_eq!(p0.c, PIECE_SIZE as u32);
        assert_eq!(p1.c, PIECE_SIZE as u32 / 2);

        // ETag mismatch invalidates resume.
        let other = Some("\"xyz\"".to_string());
        assert!(load_piece_meta(&part, PIECE_SIZE * 3, &other).is_none());

        // Cleanup
        delete_chunk_meta(&part);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
