use serde_json::{Map, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use super::cookie_store::CookieStore;
use super::error_code::classify_error;
use super::events::{EngineEvent, EventBroadcaster};
use super::http;
use super::media;
use super::options::EngineOptions;
use super::routing::{resolve_routing, TaskRoutingRule};
use super::session::SessionManager;
use super::speed_limiter::{parse_speed_limit, SpeedLimiter};
use super::task::{
    generate_gid, ChunkProgress, DownloadFile, DownloadTask, FileUri, PeerInfo, TaskKind,
    TaskStatus,
};
use super::torrent::{self, TorrentEngine};
use super::upload::UploadFileSnapshot;
use std::collections::HashSet;

const MAGNET_METADATA_ATTEMPT_TIMEOUT_SECS: u64 = 60;
const MAGNET_METADATA_RETRY_DELAY_SECS: u64 = 15;

static WORKER_EPOCH: AtomicU64 = AtomicU64::new(1);

fn next_worker_epoch() -> u64 {
    WORKER_EPOCH.fetch_add(1, Ordering::Relaxed)
}

struct ActiveDownload {
    epoch: u64,
    cancel_token: CancellationToken,
    total: Arc<AtomicU64>,
    completed: Arc<AtomicU64>,
    speed: Arc<AtomicU64>,
    connections: Arc<AtomicU32>,
    chunk_completed: Vec<Arc<AtomicU64>>,
    adopted_filename: Arc<parking_lot::Mutex<Option<String>>>,
    metalink_files: Vec<(usize, Counters)>,
}

/// Shared per-worker atomics plus cancellation hooks, cloned into both the
/// ActiveDownload registry entry and the protocol worker
#[derive(Clone)]
struct Counters {
    cancel_token: CancellationToken,
    total: Arc<AtomicU64>,
    completed: Arc<AtomicU64>,
    speed: Arc<AtomicU64>,
    connections: Arc<AtomicU32>,
}

impl Counters {
    fn new(total: u64, connections: u32) -> Self {
        Self {
            cancel_token: CancellationToken::new(),
            total: Arc::new(AtomicU64::new(total)),
            completed: Arc::new(AtomicU64::new(0)),
            speed: Arc::new(AtomicU64::new(0)),
            connections: Arc::new(AtomicU32::new(connections)),
        }
    }

    fn to_active(
        &self,
        epoch: u64,
        chunk_completed: Vec<Arc<AtomicU64>>,
        adopted_filename: Arc<parking_lot::Mutex<Option<String>>>,
    ) -> ActiveDownload {
        ActiveDownload {
            epoch,
            cancel_token: self.cancel_token.clone(),
            total: self.total.clone(),
            completed: self.completed.clone(),
            speed: self.speed.clone(),
            connections: self.connections.clone(),
            chunk_completed,
            adopted_filename,
            metalink_files: Vec::new(),
        }
    }
}

/// Per-chunk progress with a fair-share baseline denominator. With
/// work-stealing, each counter holds total bytes downloaded by worker i
/// across all pieces it pulled — not a fixed slice — so clamp so percent
/// never exceeds 100%
fn chunk_progress(chunks: &[Arc<AtomicU64>], total: u64) -> Vec<ChunkProgress> {
    let split_count = chunks.len() as u64;
    let chunk_size = total / split_count;
    chunks
        .iter()
        .enumerate()
        .map(|(i, cc)| {
            let baseline = if i as u64 == split_count - 1 {
                total - chunk_size * (split_count - 1)
            } else {
                chunk_size
            };
            let completed = cc.load(Ordering::Relaxed);
            ChunkProgress {
                completed,
                total: completed.max(baseline),
            }
        })
        .collect()
}

/// Epoch-checked completion bookkeeping shared by every protocol worker:
/// sync the task record from the counters, build the single-file vec and
/// emit DownloadComplete on success, map cancellation to Paused, mark
/// anything else as Error, then drop the active entry if it's still this
/// worker's. `on_found` runs for both outcomes (http's chunk snapshot),
/// `on_ok` applies protocol extras and returns the file completed-length,
/// `on_err` classifies the error (http also evicts stale cookies)
#[allow(clippy::too_many_arguments)]
async fn finish_task(
    tasks: &Arc<RevLock>,
    active: &Arc<RwLock<HashMap<String, ActiveDownload>>>,
    events: &EventBroadcaster,
    gid: &str,
    worker_epoch: u64,
    proto_label: &str,
    counters: &Counters,
    result: Result<std::path::PathBuf, String>,
    on_found: impl FnOnce(&mut DownloadTask),
    on_ok: impl FnOnce(&mut DownloadTask, &Path) -> u64,
    on_err: impl FnOnce(&mut DownloadTask, &str) -> super::error_code::ErrorCode,
) {
    let mut tasks_guard = tasks.write().await;
    let is_current = active.read().await.get(gid).map(|ad| ad.epoch) == Some(worker_epoch);
    if let Some(task) = tasks_guard
        .iter_mut()
        .find(|t| t.gid == gid)
        .filter(|_| is_current)
    {
        task.total_length = counters.total.load(Ordering::Relaxed);
        task.completed_length = counters.completed.load(Ordering::Relaxed);
        task.download_speed = 0;
        on_found(task);

        match result {
            Ok(path) => {
                let file_completed = on_ok(task, &path);
                tracing::info!(
                    "[task:{}] {} download complete: {}",
                    gid,
                    proto_label,
                    path.display()
                );
                task.status = TaskStatus::Complete;
                task.files = vec![DownloadFile {
                    index: "1".to_string(),
                    path: path.to_string_lossy().to_string(),
                    length: task.total_length.to_string(),
                    completed_length: file_completed.to_string(),
                    selected: "true".to_string(),
                    uris: task
                        .uris
                        .iter()
                        .map(|u| FileUri {
                            uri: u.clone(),
                            status: "used".to_string(),
                        })
                        .collect(),
                }];
                events.send(EngineEvent::DownloadComplete {
                    gid: gid.to_string(),
                });
            }
            Err(e) => {
                if e.contains("cancelled") {
                    if task.status == TaskStatus::Active {
                        task.status = TaskStatus::Paused;
                        events.send(EngineEvent::DownloadPause {
                            gid: gid.to_string(),
                        });
                    }
                } else {
                    tracing::error!("[{}] Download failed for {}: {}", proto_label, gid, e);
                    task.status = TaskStatus::Error;
                    task.error_code = Some(on_err(task, &e).to_string());
                    task.error_message = Some(e);
                    events.send(EngineEvent::DownloadError {
                        gid: gid.to_string(),
                    });
                }
            }
        }
    }
    drop(tasks_guard);

    let mut active_guard = active.write().await;
    if active_guard.get(gid).map(|ad| ad.epoch) == Some(worker_epoch) {
        active_guard.remove(gid);
    }
}

async fn metalink_finish(
    tasks: &Arc<RevLock>,
    active: &Arc<RwLock<HashMap<String, ActiveDownload>>>,
    events: &EventBroadcaster,
    gid: &str,
    worker_epoch: u64,
    file_counters: Vec<(usize, Counters)>,
    results: Vec<(usize, String, Result<std::path::PathBuf, String>)>,
) {
    let mut tasks_guard = tasks.write().await;
    let is_current = active.read().await.get(gid).map(|ad| ad.epoch) == Some(worker_epoch);
    if let Some(task) = tasks_guard
        .iter_mut()
        .find(|t| t.gid == gid)
        .filter(|_| is_current)
    {
        for (idx, c) in &file_counters {
            if let Some(f) = task.files.get_mut(*idx) {
                f.completed_length = c.completed.load(Ordering::Relaxed).to_string();
                let t = c.total.load(Ordering::Relaxed);
                if t > 0 {
                    f.length = t.to_string();
                }
            }
        }
        for (idx, _, r) in &results {
            if let (Ok(path), Some(f)) = (r, task.files.get_mut(*idx)) {
                f.path = path.to_string_lossy().to_string();
            }
        }
        metalink_rollup_totals(task);

        if task.status == TaskStatus::Active {
            let failures: Vec<(&str, &String)> = results
                .iter()
                .filter_map(|(_, name, r)| match r {
                    Err(e) if !e.contains("cancelled") => Some((name.as_str(), e)),
                    _ => None,
                })
                .collect();
            task.download_speed = 0;
            let completed_any = results.iter().any(|(_, _, r)| r.is_ok());
            if failures.is_empty() && completed_any {
                task.status = TaskStatus::Complete;
                events.send(EngineEvent::DownloadComplete {
                    gid: gid.to_string(),
                });
            } else if failures.is_empty() {
                // every in-flight file was cancelled (pause/stop), none finished —
                // not a completion, leave the batch paused so it can resume
                task.status = TaskStatus::Paused;
            } else {
                let names: Vec<&str> = failures.iter().map(|(n, _)| *n).collect();
                let first_err = failures[0].1.clone();
                task.status = TaskStatus::Paused;
                task.error_code = Some(classify_error(&first_err, "http").to_string());
                task.error_message = Some(format!(
                    "{} file(s) failed: {} — {}",
                    failures.len(),
                    names.join(", "),
                    first_err
                ));
                events.send(EngineEvent::DownloadError {
                    gid: gid.to_string(),
                });
            }
        }
    }
    drop(tasks_guard);

    let mut active_guard = active.write().await;
    if active_guard.get(gid).map(|ad| ad.epoch) == Some(worker_epoch) {
        active_guard.remove(gid);
    }
}

/// Sum the selected files' byte totals into the aggregate task fields.
fn metalink_rollup_totals(task: &mut DownloadTask) {
    let mut total = 0u64;
    let mut completed = 0u64;
    for f in task.files.iter() {
        if f.selected == "false" {
            continue;
        }
        total += f.length.parse::<u64>().unwrap_or(0);
        completed += f.completed_length.parse::<u64>().unwrap_or(0);
    }
    task.total_length = total;
    task.completed_length = completed;
}

fn metalink_checksums(options: &Map<String, Value>) -> Vec<String> {
    options
        .get("metalink-checksums")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .map(|v| v.as_str().unwrap_or("").to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn apply_select_file(files: &mut [DownloadFile], options: &Map<String, Value>) {
    let raw = options
        .get("select-file")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .unwrap_or("");
    if raw.is_empty() {
        for f in files.iter_mut() {
            f.selected = "true".to_string();
        }
        return;
    }
    let wanted: std::collections::HashSet<usize> = raw
        .split(',')
        .filter_map(|s| s.trim().parse::<usize>().ok())
        .filter(|i| *i >= 1)
        .map(|i| i - 1)
        .collect();
    for (i, f) in files.iter_mut().enumerate() {
        f.selected = if wanted.contains(&i) { "true" } else { "false" }.to_string();
    }
}

struct RevLock {
    inner: RwLock<Vec<DownloadTask>>,
    rev: AtomicU64,
}

impl RevLock {
    fn new(tasks: Vec<DownloadTask>) -> Self {
        Self {
            inner: RwLock::new(tasks),
            rev: AtomicU64::new(0),
        }
    }

    fn rev(&self) -> u64 {
        self.rev.load(Ordering::Relaxed)
    }

    async fn read(&self) -> tokio::sync::RwLockReadGuard<'_, Vec<DownloadTask>> {
        self.inner.read().await
    }

    async fn write(&self) -> RevWriteGuard<'_> {
        RevWriteGuard {
            guard: self.inner.write().await,
            rev: &self.rev,
        }
    }
}

struct RevWriteGuard<'a> {
    guard: tokio::sync::RwLockWriteGuard<'a, Vec<DownloadTask>>,
    rev: &'a AtomicU64,
}

impl std::ops::Deref for RevWriteGuard<'_> {
    type Target = Vec<DownloadTask>;
    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}

impl std::ops::DerefMut for RevWriteGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.guard
    }
}

impl Drop for RevWriteGuard<'_> {
    fn drop(&mut self) {
        self.rev.fetch_add(1, Ordering::Relaxed);
    }
}

pub struct TaskManager {
    tasks: Arc<RevLock>,
    saved_rev: AtomicU64,
    active_downloads: Arc<RwLock<HashMap<String, ActiveDownload>>>,
    torrent_ids: Arc<RwLock<HashMap<String, usize>>>,
    pending_magnets: Arc<RwLock<HashSet<String>>>,
    purged_hashes: Arc<RwLock<HashSet<String>>>,
    options: Arc<RwLock<EngineOptions>>,
    events: EventBroadcaster,
    session: SessionManager,
    torrent_engine: Arc<RwLock<Option<TorrentEngine>>>,
    global_speed_limiter: Arc<SpeedLimiter>,
    cookie_store: Arc<CookieStore>,
}

/// Extract filename hint from the first HTTP URI by parsing URL path
fn extract_filename_from_uri(uris: &[String]) -> String {
    if uris.is_empty() {
        return String::new();
    }

    // Try to extract path component from URI
    let uri = &uris[0];

    // Find the path after the domain (skip scheme and host)
    if let Some(start) = uri.find("://") {
        let after_scheme = &uri[start + 3..];
        // Skip the host part (up to the first / or ? or #)
        if let Some(path_start) = after_scheme.find('/') {
            let path = &after_scheme[path_start..];
            // Extract last path segment, strip query params and fragments
            if let Some(file_part) = path.split('/').next_back() {
                if !file_part.is_empty() {
                    let file_name = file_part
                        .split('?')
                        .next()
                        .unwrap_or("")
                        .split('#')
                        .next()
                        .unwrap_or("");
                    if !file_name.is_empty() {
                        return file_name.to_string();
                    }
                }
            }
        }
    }

    String::new()
}

fn header_contains_cookie(value: &Value) -> bool {
    let lines: Vec<&str> = if let Some(s) = value.as_str() {
        s.split('\n').collect()
    } else if let Some(arr) = value.as_array() {
        arr.iter().filter_map(|v| v.as_str()).collect()
    } else {
        return false;
    };
    lines.iter().any(|l| {
        let lower = l.trim().to_ascii_lowercase();
        lower.starts_with("cookie:") || lower.starts_with("cookie ")
    })
}

/// True when `task` is the still-active magnet task the metadata resolver
/// was spawned for
fn is_live_magnet(task: &DownloadTask, gid: &str, uri: &str) -> bool {
    task.gid == gid
        && task.kind == TaskKind::Torrent
        && task.status == TaskStatus::Active
        && task.uris.iter().any(|u| u == uri)
}

/// Extract `host=...` from the cloudflare-challenge marker error so the
/// manager can evict the matching saved entry on re-detection
fn parse_cf_host(msg: &str) -> Option<String> {
    let key = "host=";
    let start = msg.find(key)? + key.len();
    let rest = &msg[start..];
    let end = rest.find(|c: char| c.is_whitespace()).unwrap_or(rest.len());
    let host = rest[..end].trim();
    if host.is_empty() {
        None
    } else {
        Some(host.to_lowercase())
    }
}

impl TaskManager {
    pub async fn new(
        config_dir: &Path,
        options: EngineOptions,
        events: EventBroadcaster,
    ) -> Result<Self, String> {
        let session = SessionManager::new(config_dir);
        let mut saved_tasks = session.load();

        let mut purged_hashes: HashSet<String> = HashSet::new();
        if options.purge_record_on_start() {
            for task in saved_tasks.iter() {
                if task.status.is_stopped() {
                    if let Some(hash) = &task.info_hash {
                        purged_hashes.insert(hash.clone());
                    }
                }
            }
            saved_tasks.retain(|t| !t.status.is_stopped());
        }

        let output_dir = options.dir();
        let tuning = super::torrent::BtTuning {
            max_outstanding_per_peer: options.bt_max_outstanding_per_peer(),
            max_peers_per_torrent: options.bt_max_peers_per_torrent(),
            upload_rate_limit: options.bt_upload_rate_limit(),
            enable_upnp: Some(options.bt_enable_upnp()),
            upnp_lease: options.bt_upnp_lease(),
            encryption_policy: Some(options.bt_encryption_policy().to_string()),
            listen_ipv6: Some(options.bt_listen_v6()),
            enable_lsd: Some(options.bt_enable_lsd()),
        };
        let torrent_engine = TorrentEngine::new_with_tuning(Path::new(&output_dir), tuning)
            .await
            .map_err(|e| {
                tracing::warn!("Torrent engine init failed (non-fatal): {}", e);
                e
            })
            .ok();

        let global_speed_limiter =
            Arc::new(SpeedLimiter::new(options.max_overall_download_limit()));

        // Set up the DNS-over-HTTPS resolver (or system DNS) before any task
        // can open a connection. This applies process-wide to every
        // risuko-http client through the global resolver hook
        super::dns::apply_from_options(&options.global);

        let manager = Self {
            tasks: Arc::new(RevLock::new(saved_tasks)),
            // MAX so the first auto-save always runs once (rev starts at 0)
            saved_rev: AtomicU64::new(u64::MAX),
            active_downloads: Arc::new(RwLock::new(HashMap::new())),
            torrent_ids: Arc::new(RwLock::new(HashMap::new())),
            pending_magnets: Arc::new(RwLock::new(HashSet::new())),
            purged_hashes: Arc::new(RwLock::new(purged_hashes)),
            options: Arc::new(RwLock::new(options)),
            events,
            session,
            torrent_engine: Arc::new(RwLock::new(torrent_engine)),
            global_speed_limiter,
            cookie_store: Arc::new(CookieStore::new(config_dir)),
        };

        if manager.restore_torrent_mappings().await {
            manager.purged_hashes.write().await.clear();
        }

        Ok(manager)
    }

    async fn restore_torrent_mappings(&self) -> bool {
        let te_guard = self.torrent_engine.read().await;
        let Some(ref te) = *te_guard else {
            return false;
        };

        let purged_hashes = self.purged_hashes.read().await.clone();

        let managed = te.list_managed_torrents();
        if managed.is_empty() {
            return true;
        }

        let mut orphans: Vec<(usize, String)> = Vec::new();
        let mut cleanup_failed = false;

        {
            let mut tasks = self.tasks.write().await;
            let mut ids = self.torrent_ids.write().await;

            for (torrent_id, info_hash) in &managed {
                let mut matched = false;
                for task in tasks.iter_mut() {
                    if task.kind == TaskKind::Torrent
                        && task.info_hash.as_deref() == Some(info_hash.as_str())
                        && task.status != TaskStatus::Removed
                    {
                        ids.insert(task.gid.clone(), *torrent_id);
                        if task.status == TaskStatus::Paused {
                            task.status = TaskStatus::Active;
                        }
                        tracing::info!(
                            "Restored torrent mapping: gid={} -> torrent_id={} ({})",
                            task.gid,
                            torrent_id,
                            info_hash
                        );
                        matched = true;
                        break;
                    }
                }
                if !matched {
                    orphans.push((*torrent_id, info_hash.clone()));
                }
            }

            tracing::info!(
                "Restored {} torrent mappings out of {} persisted torrents ({} orphan)",
                ids.len(),
                managed.len(),
                orphans.len()
            );
        }

        // Purge orphan torrents (persisted but no live task). Without this the
        // torrent engine auto-resumes them on startup and writes files even
        // though the user deleted or never had the task in Motrix. Orphans from
        // purge_record_on_start keep their files
        for (torrent_id, info_hash) in orphans {
            let delete_files = !purged_hashes.contains(&info_hash);
            let removal_result = te.remove(torrent_id, delete_files).await;
            let removal_failed = removal_result.is_err();
            match removal_result {
                Ok(()) => tracing::info!(
                    "Purged orphan persisted torrent: torrent_id={} ({}) [delete_files={}]",
                    torrent_id,
                    info_hash,
                    delete_files
                ),
                Err(e) => tracing::warn!(
                    "Failed to purge orphan torrent torrent_id={} ({}): {}",
                    torrent_id,
                    info_hash,
                    e
                ),
            }
            if removal_failed {
                cleanup_failed = true;
            }
        }

        !cleanup_failed
    }

    /// Resolve download directory and tag for a new task by evaluating routing
    /// rules against the inferred output filename
    async fn resolve_routing_for_task(
        &self,
        options: &Map<String, Value>,
        filename_hint: &str,
    ) -> (String, Option<String>) {
        let opts_guard = self.options.read().await;
        let merged = opts_guard.merge_task_options(options);
        let raw_dir = merged
            .get("dir")
            .and_then(|v| v.as_str())
            .unwrap_or(".")
            .to_string();
        let rules = opts_guard.task_routing_rules();
        let file_category_dirs = opts_guard.file_category_dirs();
        drop(opts_guard);

        let decision = resolve_routing(&rules, filename_hint, &raw_dir, &file_category_dirs);
        (decision.dir, decision.tag)
    }

    fn send_download_start(&self, gid: &str) {
        self.events.send(EngineEvent::DownloadStart {
            gid: gid.to_string(),
        });
    }

    /// Common tail for the queue-based protocols: honor a per-task `pause`
    /// option (explicitly set, not from global defaults), enqueue the task,
    /// kick the scheduler, and emit DownloadStart
    async fn enqueue(&self, mut task: DownloadTask) -> Result<String, String> {
        let gid = task.gid.clone();
        let pause = task
            .options
            .get("pause")
            .and_then(|v| v.as_bool().or_else(|| v.as_str().map(|s| s == "true")))
            .unwrap_or(false);

        // Scheduled start
        let scheduled = task
            .options
            .get("risuko-start-at")
            .and_then(|v| {
                v.as_u64()
                    .or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok()))
            })
            .filter(|&ts| ts > 0);
        if let Some(ts) = scheduled {
            task.start_at = Some(ts);
            task.status = TaskStatus::Scheduled;
        }

        self.tasks.write().await.push(task);

        if scheduled.is_none() && !pause {
            self.try_start_next().await;
        }

        self.send_download_start(&gid);

        Ok(gid)
    }

    pub async fn add_http_task(
        &self,
        uris: Vec<String>,
        options: Map<String, Value>,
    ) -> Result<String, String> {
        if let Some(magnet) = uris.iter().find(|u| torrent::is_magnet_uri(u)) {
            return self.add_magnet_task(magnet, options).await;
        }

        if uris.len() == 1 && super::metalink::url_hints_metalink(&uris[0]) {
            let merged = self.options.read().await.merge_task_options(&options);
            if let Ok(bytes) = http::fetch_for_metalink_probe(&uris[0], &merged).await {
                // strict UTF-8 to match add_metalink_task's own check, so a
                // non-UTF-8 body is never classified as metalink and rejected later
                if std::str::from_utf8(&bytes)
                    .ok()
                    .is_some_and(|text| super::metalink::parse(text).is_ok())
                {
                    tracing::info!("[metalink] following metalink URL: {}", uris[0]);
                    match self.add_metalink_task(bytes, options.clone()).await {
                        Ok(gid) => return Ok(gid),
                        Err(e) => tracing::warn!(
                            "[metalink] {} parsed but task creation failed, falling back to HTTP: {e}",
                            uris[0]
                        ),
                    }
                }
            }
        }

        let gid = generate_gid();
        tracing::info!("[task:{}] Adding HTTP task, uris={:?}", gid, uris);
        let out = options
            .get("out")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // Derive filename hint from URI if out is empty
        let filename_hint = if out.is_empty() {
            extract_filename_from_uri(&uris)
        } else {
            out.clone()
        };

        let (dir, tag) = self
            .resolve_routing_for_task(&options, &filename_hint)
            .await;

        self.enqueue(DownloadTask::new_http(gid, uris, dir, tag, options))
            .await
    }

    pub async fn add_media_task(
        &self,
        uri: &str,
        options: Map<String, Value>,
    ) -> Result<String, String> {
        let gid = generate_gid();
        tracing::info!("[task:{}] Adding media task, uri={}", gid, uri);
        let out = options
            .get("out")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // Use a neutral hint if out is empty to avoid misclassifying audio-only tasks
        let filename_hint = if out.is_empty() {
            String::new()
        } else {
            out.clone()
        };

        let (dir, tag) = self
            .resolve_routing_for_task(&options, &filename_hint)
            .await;

        self.enqueue(DownloadTask::new_media(
            gid,
            uri.to_string(),
            dir,
            tag,
            options,
        ))
        .await
    }

    pub async fn add_torrent_task(
        &self,
        torrent_data: Vec<u8>,
        options: Map<String, Value>,
    ) -> Result<String, String> {
        let gid = generate_gid();
        let merged = self.options.read().await.merge_task_options(&options);
        let out = options
            .get("out")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // Use .torrent extension as hint if out is empty
        let filename_hint = if out.is_empty() {
            "download.torrent".to_string()
        } else {
            out.clone()
        };

        let (dir, tag) = self
            .resolve_routing_for_task(&options, &filename_hint)
            .await;

        let mut task = DownloadTask::new_torrent(gid.clone(), dir.clone(), tag, options.clone());

        // Add to torrent engine
        let te_guard = self.torrent_engine.read().await;
        if let Some(ref te) = *te_guard {
            match te.add_torrent_bytes(&torrent_data, &merged).await {
                Ok(handle) => {
                    tracing::info!(
                        "Torrent task {} added: id={}, info_hash={:?}",
                        gid,
                        handle.id,
                        handle.info_hash
                    );
                    self.torrent_ids
                        .write()
                        .await
                        .insert(gid.clone(), handle.id);
                    task.info_hash = handle.info_hash;
                    task.info_hash_v2 = handle.info_hash_v2;
                    task.meta_version = handle.meta_version;
                    task.status = TaskStatus::Active;
                }
                Err(e) => {
                    tracing::error!("Torrent task {} failed to add: {}", gid, e);
                    task.status = TaskStatus::Error;
                    task.error_code = Some(classify_error(&e, "torrent").to_string());
                    task.error_message = Some(e);
                }
            }
        } else {
            tracing::error!("Torrent task {} failed: engine not available", gid);
            task.status = TaskStatus::Error;
            task.error_code = Some(super::error_code::ErrorCode::ENGINE_NOT_RUNNING.to_string());
            task.error_message = Some("Torrent engine not available".to_string());
        }
        drop(te_guard);

        self.tasks.write().await.push(task);
        self.send_download_start(&gid);

        Ok(gid)
    }

    pub async fn add_metalink_task(
        &self,
        meta4: Vec<u8>,
        options: Map<String, Value>,
    ) -> Result<String, String> {
        let xml = String::from_utf8(meta4).map_err(|e| format!("metalink is not UTF-8: {e}"))?;
        let files = super::metalink::parse(&xml)?;

        let gid = generate_gid();
        let filename_hint = files.first().map(|f| f.name.clone()).unwrap_or_default();
        let (dir, tag) = self
            .resolve_routing_for_task(&options, &filename_hint)
            .await;

        let mut download_files = Vec::with_capacity(files.len());
        let mut checksums = Vec::with_capacity(files.len());
        for (i, f) in files.iter().enumerate() {
            download_files.push(DownloadFile {
                index: (i + 1).to_string(),
                path: format!("{}/{}", dir, f.name),
                length: "0".to_string(),
                completed_length: "0".to_string(),
                selected: "true".to_string(),
                uris: f
                    .uris
                    .iter()
                    .map(|u| FileUri {
                        uri: u.clone(),
                        status: "waiting".to_string(),
                    })
                    .collect(),
            });
            checksums.push(Value::String(
                f.checksum
                    .as_ref()
                    .map(|c| format!("{}:{}", c.algo.name(), c.hex))
                    .unwrap_or_default(),
            ));
        }

        let mut options = options;
        apply_select_file(&mut download_files, &options);
        options.insert("metalink-checksums".to_string(), Value::Array(checksums));

        tracing::info!(
            "[task:{}] Adding Metalink task, {} file(s)",
            gid,
            download_files.len()
        );
        self.enqueue(DownloadTask::new_metalink(
            gid,
            dir,
            tag,
            options,
            download_files,
        ))
        .await
    }

    pub async fn add_magnet_task(
        &self,
        magnet_uri: &str,
        options: Map<String, Value>,
    ) -> Result<String, String> {
        let gid = generate_gid();
        let merged = self.options.read().await.merge_task_options(&options);
        let out = options
            .get("out")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // Use magnet extension as hint if out is empty
        let filename_hint = if out.is_empty() {
            "download.magnet".to_string()
        } else {
            out.clone()
        };

        let (dir, tag) = self
            .resolve_routing_for_task(&options, &filename_hint)
            .await;

        let mut task = DownloadTask::new_torrent(gid.clone(), dir.clone(), tag, options.clone());
        task.uris = vec![magnet_uri.to_string()];

        let should_spawn_resolver = match torrent::inspect_magnet(magnet_uri) {
            Ok(info) => {
                task.info_hash = Some(info.info_hash);
                task.info_hash_v2 = info.info_hash_v2;
                task.bt_name = info.display_name;
                task.status = TaskStatus::Active;
                true
            }
            Err(e) => {
                task.status = TaskStatus::Error;
                task.error_code = Some(classify_error(&e, "torrent").to_string());
                task.error_message = Some(e);
                false
            }
        };

        if should_spawn_resolver && self.torrent_engine.read().await.is_none() {
            task.status = TaskStatus::Error;
            task.error_code = Some(super::error_code::ErrorCode::ENGINE_NOT_RUNNING.to_string());
            task.error_message = Some("Torrent engine not available".to_string());
        }

        let should_start_resolver = task.status == TaskStatus::Active;

        self.tasks.write().await.push(task);
        if should_start_resolver {
            self.spawn_magnet_metadata_resolver(
                gid.clone(),
                magnet_uri.to_string(),
                merged.clone(),
            )
            .await;
        }
        self.send_download_start(&gid);

        Ok(gid)
    }

    pub async fn resolve_magnet_metadata(
        &self,
        magnet_uri: &str,
        options: Map<String, Value>,
        timeout_secs: u64,
    ) -> Result<Vec<torrent::TorrentFileInfo>, String> {
        let merged = self.options.read().await.merge_task_options(&options);
        let te_guard = self.torrent_engine.read().await;
        if let Some(ref te) = *te_guard {
            te.resolve_magnet(magnet_uri, &merged, timeout_secs).await
        } else {
            Err("Torrent engine not available".to_string())
        }
    }

    async fn spawn_magnet_metadata_resolver(
        &self,
        gid: String,
        magnet_uri: String,
        options: Map<String, Value>,
    ) {
        {
            let mut guard = self.pending_magnets.write().await;
            if !guard.insert(gid.clone()) {
                return;
            }
        }

        let pending = self.pending_magnets.clone();
        let tasks = self.tasks.clone();
        let torrent_ids = self.torrent_ids.clone();
        let torrent_engine = self.torrent_engine.clone();
        let events = self.events.clone();

        tokio::spawn(async move {
            loop {
                let still_active = {
                    let guard = tasks.read().await;
                    guard
                        .iter()
                        .any(|task| is_live_magnet(task, &gid, &magnet_uri))
                };
                if !still_active {
                    break;
                }

                let engine = torrent_engine.read().await.clone();
                let Some(engine) = engine else {
                    let mut guard = tasks.write().await;
                    if let Some(task) = guard
                        .iter_mut()
                        .find(|task| is_live_magnet(task, &gid, &magnet_uri))
                    {
                        task.status = TaskStatus::Error;
                        task.error_code =
                            Some(super::error_code::ErrorCode::ENGINE_NOT_RUNNING.to_string());
                        task.error_message = Some("Torrent engine not available".to_string());
                        events.send(EngineEvent::DownloadError { gid: gid.clone() });
                    }
                    break;
                };

                match engine
                    .resolve_and_add_magnet(
                        &magnet_uri,
                        &options,
                        MAGNET_METADATA_ATTEMPT_TIMEOUT_SECS,
                    )
                    .await
                {
                    Ok(handle) => {
                        let mut attached = false;
                        {
                            let mut guard = tasks.write().await;
                            if let Some(task) = guard
                                .iter_mut()
                                .find(|task| is_live_magnet(task, &gid, &magnet_uri))
                            {
                                if let Some(info_hash) = handle.info_hash.clone() {
                                    task.info_hash = Some(info_hash);
                                }
                                task.info_hash_v2 = handle.info_hash_v2.clone();
                                task.meta_version = handle.meta_version.clone();
                                task.error_code = None;
                                task.error_message = None;
                                attached = true;
                            }
                        }

                        if attached {
                            torrent_ids.write().await.insert(gid.clone(), handle.id);
                        } else {
                            let _ = engine.remove(handle.id, false).await;
                        }
                        break;
                    }
                    Err(e) => {
                        if !is_retryable_magnet_resolution_error(&e) {
                            let mut guard = tasks.write().await;
                            if let Some(task) = guard
                                .iter_mut()
                                .find(|task| is_live_magnet(task, &gid, &magnet_uri))
                            {
                                task.status = TaskStatus::Error;
                                task.error_code = Some(classify_error(&e, "torrent").to_string());
                                task.error_message = Some(e);
                                events.send(EngineEvent::DownloadError { gid: gid.clone() });
                            }
                            break;
                        }
                        tokio::time::sleep(Duration::from_secs(MAGNET_METADATA_RETRY_DELAY_SECS))
                            .await;
                    }
                }
            }

            pending.write().await.remove(&gid);
        });
    }

    pub async fn add_ed2k_task(
        &self,
        uri: &str,
        options: Map<String, Value>,
    ) -> Result<String, String> {
        let link = super::ed2k::parse_ed2k_link(uri)?;
        let gid = generate_gid();
        let file_name = link.file_name.clone();
        let file_size = link.file_size;
        let (dir, tag) = self.resolve_routing_for_task(&options, &file_name).await;

        self.enqueue(DownloadTask::new_ed2k(
            gid,
            uri.to_string(),
            file_name,
            file_size,
            dir,
            tag,
            options,
        ))
        .await
    }

    pub async fn add_m3u8_task(
        &self,
        uri: &str,
        options: Map<String, Value>,
    ) -> Result<String, String> {
        let gid = generate_gid();

        // Infer output filename: strip .m3u8 → .ts
        let out = options
            .get("out")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| infer_m3u8_output_name(uri));
        let (dir, tag) = self.resolve_routing_for_task(&options, &out).await;

        self.enqueue(DownloadTask::new_m3u8(
            gid,
            uri.to_string(),
            out,
            dir,
            tag,
            options,
        ))
        .await
    }

    pub async fn add_ftp_task(
        &self,
        uri: &str,
        options: Map<String, Value>,
    ) -> Result<String, String> {
        let gid = generate_gid();
        let out = options
            .get("out")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // Derive filename hint from FTP URI if out is empty
        let filename_hint = if out.is_empty() {
            extract_filename_from_uri(&[uri.to_string()])
        } else {
            out.clone()
        };

        let (dir, tag) = self
            .resolve_routing_for_task(&options, &filename_hint)
            .await;

        self.enqueue(DownloadTask::new_ftp(
            gid,
            uri.to_string(),
            dir,
            tag,
            options,
        ))
        .await
    }

    /// Add a task for one of the legacy P2P / IPC protocols (ADC, Gnutella,
    /// G2, giFT). The protocol module's URI parser extracts an output filename
    /// and size hint when the URI carries them
    pub async fn add_legacy_p2p_task(
        &self,
        kind: TaskKind,
        uri: &str,
        options: Map<String, Value>,
    ) -> Result<String, String> {
        // Resolve filename + size from the URI when possible
        let (out_hint, size_hint) = match kind {
            TaskKind::Adc => super::adc::parse_dchub_file_uri(uri)
                .map(|p| (p.file_name, p.file_size))
                .unwrap_or_default(),
            TaskKind::Gnutella => super::gnutella::parse_gnutella_uri(uri)
                .map(|p| (p.file_name, p.file_size))
                .unwrap_or_default(),
            TaskKind::G2 => super::g2::parse_g2_uri(uri)
                .map(|p| (p.file_name, p.file_size))
                .unwrap_or_default(),
            TaskKind::Gift => super::gift::parse_gift_uri(uri)
                .map(|p| (super::gift::extract_gift_name(&p.inner), 0u64))
                .unwrap_or_default(),
            _ => (String::new(), 0),
        };

        let gid = generate_gid();
        let (dir, tag) = self.resolve_routing_for_task(&options, &out_hint).await;

        self.enqueue(DownloadTask::new_simple_protocol(
            gid,
            kind,
            uri.to_string(),
            out_hint,
            size_hint,
            dir,
            tag,
            options,
        ))
        .await
    }

    // -- Routing rule management --

    pub async fn list_routing_rules(&self) -> Vec<TaskRoutingRule> {
        self.options.read().await.task_routing_rules()
    }

    pub async fn add_routing_rule(
        &self,
        mut rule: TaskRoutingRule,
    ) -> Result<TaskRoutingRule, String> {
        if rule.id.is_empty() {
            rule.id = uuid::Uuid::new_v4().to_string();
        }
        let mut opts = self.options.write().await;
        let mut rules = opts.task_routing_rules();
        rules.push(rule.clone());
        opts.set(
            "task-routing-rules".to_string(),
            serde_json::to_value(rules).unwrap_or_default(),
        );
        Ok(rule)
    }

    pub async fn update_routing_rule(&self, rule: TaskRoutingRule) -> Result<(), String> {
        let mut opts = self.options.write().await;
        let mut rules = opts.task_routing_rules();
        let pos = rules.iter().position(|r| r.id == rule.id);
        match pos {
            Some(idx) => {
                rules[idx] = rule;
                opts.set(
                    "task-routing-rules".to_string(),
                    serde_json::to_value(rules).unwrap_or_default(),
                );
                Ok(())
            }
            None => Err("Rule not found".to_string()),
        }
    }

    pub async fn remove_routing_rule(&self, id: &str) -> Result<(), String> {
        let mut opts = self.options.write().await;
        let mut rules = opts.task_routing_rules();
        let pos = rules.iter().position(|r| r.id == id);
        match pos {
            Some(idx) => {
                rules.remove(idx);
                opts.set(
                    "task-routing-rules".to_string(),
                    serde_json::to_value(rules).unwrap_or_default(),
                );
                Ok(())
            }
            None => Err("Rule not found".to_string()),
        }
    }

    /// Preview routing decision for a filename using current global config
    pub async fn preview_routing(&self, filename: &str) -> super::routing::RoutingDecision {
        let opts = self.options.read().await;
        let raw_dir = opts.dir();
        let rules = opts.task_routing_rules();
        let file_category_dirs = opts.file_category_dirs();
        drop(opts);
        resolve_routing(&rules, filename, &raw_dir, &file_category_dirs)
    }

    /// Common spawn machinery for ADC / Gnutella / G2 / giFT. Each protocol's `run_*_download` shares the same
    /// signature: takes the URI, dir, atomic counters, cancel hooks, and an
    /// `EngineOptions` snapshot, returns `Result<PathBuf, String>`
    fn spawn_legacy_p2p_download(&self, task: &DownloadTask) {
        let gid = task.gid.clone();
        let uri = task.uris.first().cloned().unwrap_or_default();
        let dir = task.dir.clone();
        let kind = task.kind;
        let task_options = task.options.clone();
        let events = self.events.clone();
        let tasks = self.tasks.clone();
        let active = self.active_downloads.clone();
        let options = self.options.clone();

        let proto_label = match kind {
            TaskKind::Adc => "adc",
            TaskKind::Gnutella => "gnutella",
            TaskKind::G2 => "g2",
            TaskKind::Gift => "gift",
            _ => "p2p",
        };

        let counters = Counters::new(task.total_length, 0);
        let worker_epoch = next_worker_epoch();
        tokio::spawn(async move {
            active.write().await.insert(
                gid.clone(),
                counters.to_active(
                    worker_epoch,
                    Vec::new(),
                    Arc::new(parking_lot::Mutex::new(None)),
                ),
            );

            let opts_snapshot = {
                let runtime_opts = options.read().await.clone();
                let merged = runtime_opts.merge_task_options(&task_options);
                EngineOptions { global: merged }
            };

            let c = counters.clone();
            let download_result = match kind {
                TaskKind::Adc => {
                    super::adc::run_adc_download(
                        &uri,
                        &dir,
                        &opts_snapshot,
                        c.total,
                        c.completed,
                        c.speed,
                        c.connections,
                        c.cancel_token,
                    )
                    .await
                }
                TaskKind::Gnutella => {
                    super::gnutella::run_gnutella_download(
                        &uri,
                        &dir,
                        &opts_snapshot,
                        c.total,
                        c.completed,
                        c.speed,
                        c.connections,
                        c.cancel_token,
                    )
                    .await
                }
                TaskKind::G2 => super::g2::run_g2_download(
                    &uri,
                    &dir,
                    &opts_snapshot,
                    c.total,
                    c.completed,
                    c.speed,
                    c.connections,
                    c.cancel_token,
                )
                .await
                .map_err(|e| e.to_string()),
                TaskKind::Gift => {
                    super::gift::run_gift_download(
                        &uri,
                        &dir,
                        &opts_snapshot,
                        c.total,
                        c.completed,
                        c.speed,
                        c.connections,
                        c.cancel_token,
                    )
                    .await
                }
                _ => Err("Unsupported protocol".to_string()),
            };

            finish_task(
                &tasks,
                &active,
                &events,
                &gid,
                worker_epoch,
                proto_label,
                &counters,
                download_result,
                |_| {},
                |task, _| task.total_length,
                |_, e| classify_error(e, proto_label),
            )
            .await;
        });
    }

    /// Start download workers for waiting tasks up to max concurrent limit
    async fn try_start_next(&self) {
        let (max_concurrent, options_snapshot) = {
            let options_guard = self.options.read().await;
            (
                options_guard.max_concurrent_downloads(),
                options_guard.clone(),
            )
        };
        let active_count = self.active_downloads.read().await.len();

        if active_count >= max_concurrent {
            return;
        }

        let slots = max_concurrent - active_count;
        let mut tasks = self.tasks.write().await;
        let mut started = 0;

        for task in tasks.iter_mut() {
            if started >= slots {
                break;
            }
            if task.status != TaskStatus::Waiting {
                continue;
            }
            if task.kind == TaskKind::Http && !task.uris.is_empty() {
                task.status = TaskStatus::Active;
                let mut merged = options_snapshot.merge_task_options(&task.options);
                self.apply_stored_cookies(&task.uris, &mut merged);
                self.spawn_http_download(task, merged);
                self.send_download_start(&task.gid);
                started += 1;
            } else if task.kind == TaskKind::Media && !task.uris.is_empty() {
                task.status = TaskStatus::Active;
                let mut merged = options_snapshot.merge_task_options(&task.options);
                self.apply_stored_cookies(&task.uris, &mut merged);
                self.spawn_media_download(task, merged);
                self.send_download_start(&task.gid);
                started += 1;
            } else if task.kind == TaskKind::M3u8 && !task.uris.is_empty() {
                task.status = TaskStatus::Active;
                let merged = options_snapshot.merge_task_options(&task.options);
                self.spawn_m3u8_download(task, merged);
                self.send_download_start(&task.gid);
                started += 1;
            } else if task.kind == TaskKind::Ed2k && !task.uris.is_empty() {
                task.status = TaskStatus::Active;
                self.spawn_ed2k_download(task);
                self.send_download_start(&task.gid);
                started += 1;
            } else if task.kind == TaskKind::Ftp && !task.uris.is_empty() {
                task.status = TaskStatus::Active;
                let merged = options_snapshot.merge_task_options(&task.options);
                self.spawn_ftp_download(task, merged);
                self.send_download_start(&task.gid);
                started += 1;
            } else if task.kind == TaskKind::Metalink && !task.files.is_empty() {
                task.status = TaskStatus::Active;
                let merged = options_snapshot.merge_task_options(&task.options);
                apply_select_file(&mut task.files, &task.options);
                self.spawn_metalink_download(task, merged);
                self.send_download_start(&task.gid);
                started += 1;
            } else if matches!(
                task.kind,
                TaskKind::Adc | TaskKind::Gnutella | TaskKind::G2 | TaskKind::Gift
            ) && !task.uris.is_empty()
            {
                task.status = TaskStatus::Active;
                self.spawn_legacy_p2p_download(task);
                self.send_download_start(&task.gid);
                started += 1;
            }
        }
    }

    /// Enforce the strict-priority download queue
    async fn reconcile_active_set(&self) {
        let max_concurrent = self.options.read().await.max_concurrent_downloads();
        let to_preempt: Vec<String> = {
            let tasks = self.tasks.read().await;
            let mut rank = 0usize;
            let mut preempt = Vec::new();
            for task in tasks.iter() {
                if task.kind == TaskKind::Torrent {
                    continue;
                }
                match task.status {
                    TaskStatus::Active | TaskStatus::Waiting => {
                        rank += 1;
                        if rank > max_concurrent && task.status == TaskStatus::Active {
                            preempt.push(task.gid.clone());
                        }
                    }
                    _ => {}
                }
            }
            preempt
        };
        for gid in &to_preempt {
            self.preempt(gid).await;
        }
        self.try_start_next().await;
    }

    /// Pause an active download back to Waiting (not Paused) to yield its slot to
    /// a higher-priority task. Unlike `pause`, the task stays runnable, so the next
    /// reconcile restarts it once it re-enters the top-N.
    ///
    /// Set the status to Waiting BEFORE cancelling: the download's cancellation
    /// handler only pauses a task that is still marked Active, so demoting first
    /// keeps a preempted task runnable instead of racing it into a stuck Paused
    async fn preempt(&self, gid: &str) {
        let should_cancel = {
            let mut tasks = self.tasks.write().await;
            match tasks.iter_mut().find(|t| t.gid == gid) {
                Some(task) if task.status == TaskStatus::Active => {
                    tracing::info!("[task:{}] Preempted, yielding download slot", gid);
                    task.status = TaskStatus::Waiting;
                    task.download_speed = 0;
                    task.upload_speed = 0;
                    self.events.send(EngineEvent::DownloadPause {
                        gid: gid.to_string(),
                    });
                    true
                }
                _ => false,
            }
        };
        if should_cancel {
            let active = self.active_downloads.read().await;
            if let Some(ad) = active.get(gid) {
                ad.cancel_token.cancel();
            }
        }
    }

    /// Inject stored browser cookies into the merged options for an HTTP
    /// task whose URI host has a saved entry. User-supplied cookies and
    /// User-Agent on the task itself always win; the store only fills
    /// in fields that are absent
    fn apply_stored_cookies(&self, uris: &[String], merged: &mut Map<String, Value>) {
        let Some(uri) = uris.first() else {
            return;
        };
        let Some(entry) = self.cookie_store.find_for_url(uri) else {
            tracing::debug!("apply_stored_cookies: no entry for uri={uri}");
            return;
        };

        // Cookie names and the destination host together leak more than
        // we want at info level (recognizable session keys, plus what
        // the user is downloading from). Stick to debug and elide names
        tracing::debug!(
            "apply_stored_cookies: matched stored entry (browser={}, {} cookie(s))",
            entry.browser_id,
            entry.cookies.len(),
        );

        let has_cookie = merged
            .get("cookie")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.is_empty())
            || merged
                .get("header")
                .map(header_contains_cookie)
                .unwrap_or(false);
        if !has_cookie {
            let cookie_header = super::cookie_store::cookies_to_header(&entry.cookies);
            if !cookie_header.is_empty() {
                tracing::debug!(
                    "apply_stored_cookies: injecting cookie header ({} bytes)",
                    cookie_header.len(),
                );
                merged.insert("cookie".to_string(), Value::String(cookie_header));
            }
        } else {
            tracing::debug!(
                "apply_stored_cookies: cookie already set on task, leaving stored entry untouched"
            );
        }

        let has_ua = merged
            .get("user-agent")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.is_empty());
        if !has_ua && !entry.user_agent.is_empty() {
            merged.insert(
                "user-agent".to_string(),
                Value::String(entry.user_agent.clone()),
            );
        }

        // Touch the entry so LRU eviction prefers entries actually in use
        self.cookie_store.touch(&entry.host);
    }

    pub fn cookie_store(&self) -> &Arc<CookieStore> {
        &self.cookie_store
    }

    fn spawn_http_download(&self, task: &DownloadTask, merged_options: Map<String, Value>) {
        let gid = task.gid.clone();
        // Pass the full URI list so the engine can fail over between mirrors
        // when one returns a hard error (DNS, TLS, 5xx, connect refused)
        let uris: Vec<String> = task.uris.clone();
        let dir = task.dir.clone();
        let out = task.out.clone();
        let events = self.events.clone();
        let tasks = self.tasks.clone();
        let active = self.active_downloads.clone();
        let cookie_store = self.cookie_store.clone();

        let split: u32 = merged_options
            .get("split")
            .and_then(|v| {
                v.as_u64()
                    .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            })
            .unwrap_or(1)
            .max(1) as u32;

        // split task speed limit from merged options
        let per_task_limit = merged_options
            .get("max-download-limit")
            .map(parse_speed_limit)
            .unwrap_or(0);
        let task_speed_limiter = Arc::new(SpeedLimiter::new(per_task_limit));
        let global_limiter = self.global_speed_limiter.clone();

        let counters = Counters::new(0, split);
        // split chunk progress atomics for multi-thread downloads
        let chunk_completed: Vec<Arc<AtomicU64>> =
            (0..split).map(|_| Arc::new(AtomicU64::new(0))).collect();
        let adopted_filename: Arc<parking_lot::Mutex<Option<String>>> =
            Arc::new(parking_lot::Mutex::new(None));

        let worker_epoch = next_worker_epoch();
        tokio::spawn(async move {
            active.write().await.insert(
                gid.clone(),
                counters.to_active(
                    worker_epoch,
                    chunk_completed.clone(),
                    adopted_filename.clone(),
                ),
            );

            let c = counters.clone();
            let download_result = http::run_http_download_multi(
                &uris,
                &dir,
                &out,
                &merged_options,
                c.total,
                c.completed,
                c.speed,
                c.connections,
                c.cancel_token,
                global_limiter,
                task_speed_limiter,
                chunk_completed.clone(),
                adopted_filename,
            )
            .await;

            finish_task(
                &tasks,
                &active,
                &events,
                &gid,
                worker_epoch,
                "http",
                &counters,
                download_result,
                |task| {
                    // Snapshot final per-chunk progress before the active download is removed
                    let conns = counters.connections.load(Ordering::Relaxed);
                    if chunk_completed.len() > 1 && task.total_length > 0 && conns > 1 {
                        task.chunk_progress = chunk_progress(&chunk_completed, task.total_length);
                    } else {
                        task.chunk_progress.clear();
                    }
                },
                |task, path| {
                    // Pull the final filename off disk so the task record
                    // matches any Content-Disposition rename in http.rs
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        task.out = name.to_string();
                    }
                    task.total_length
                },
                |task, e| {
                    let code = classify_error(e, "http");
                    // Drop a saved cookie entry that stopped working so the
                    // next attempt re-prompts the user instead of replaying
                    // stale credentials
                    if code == super::error_code::ErrorCode::CLOUDFLARE_CHALLENGE {
                        // Prefer the host embedded in the challenge marker so
                        // redirected URLs evict the right cookie entry
                        let lookup_url = parse_cf_host(e)
                            .map(|h| format!("https://{h}/"))
                            .unwrap_or_else(|| {
                                task.uris
                                    .first()
                                    .map(|u| u.as_str().to_owned())
                                    .unwrap_or_default()
                            });
                        if let Some(entry) = cookie_store.find_for_url(&lookup_url) {
                            if let Err(err) = cookie_store.remove(&entry.host) {
                                tracing::warn!(
                                    "[task:{gid}] cookie store remove({}) failed: {err}",
                                    entry.host
                                );
                            }
                        }
                    }
                    code
                },
            )
            .await;
        });
    }

    fn spawn_metalink_download(&self, task: &DownloadTask, merged_options: Map<String, Value>) {
        let gid = task.gid.clone();
        let dir = task.dir.clone();
        let events = self.events.clone();
        let tasks = self.tasks.clone();
        let active = self.active_downloads.clone();
        let global_limiter = self.global_speed_limiter.clone();

        let split: u32 = merged_options
            .get("split")
            .and_then(|v| {
                v.as_u64()
                    .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            })
            .unwrap_or(1)
            .max(1) as u32;
        let per_task_limit = merged_options
            .get("max-download-limit")
            .map(parse_speed_limit)
            .unwrap_or(0);
        let task_speed_limiter = Arc::new(SpeedLimiter::new(per_task_limit));

        let checksums = metalink_checksums(&task.options);
        let parent = CancellationToken::new();

        struct Spec {
            idx: usize,
            out: String,
            uris: Vec<String>,
            options: Map<String, Value>,
            counters: Counters,
            chunk: Vec<Arc<AtomicU64>>,
            adopted: Arc<parking_lot::Mutex<Option<String>>>,
        }
        let mut specs: Vec<Spec> = Vec::new();
        let mut file_counters: Vec<(usize, Counters)> = Vec::new();
        for (i, f) in task.files.iter().enumerate() {
            if f.selected == "false" {
                continue;
            }
            let len: u64 = f.length.parse().unwrap_or(0);
            let done: u64 = f.completed_length.parse().unwrap_or(0);
            if len > 0 && done >= len {
                continue;
            }
            let file_uris: Vec<String> = f.uris.iter().map(|u| u.uri.clone()).collect();
            let out = Path::new(&f.path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let mut opts = merged_options.clone();
            // mirror the HTTP/media path: fill absent cookies/UA from the store per mirror host
            self.apply_stored_cookies(&file_uris, &mut opts);
            opts.insert("out".to_string(), Value::String(out.clone()));
            if let Some(cs) = checksums.get(i) {
                if !cs.is_empty() {
                    opts.insert("checksum".to_string(), Value::String(cs.clone()));
                }
            }
            let mut counters = Counters::new(0, split);
            // child token, not the parent clone: parent pause/stop still cascades to
            // every file, but one file's stall watchdog only cancels itself
            counters.cancel_token = parent.child_token();
            let chunk: Vec<Arc<AtomicU64>> =
                (0..split).map(|_| Arc::new(AtomicU64::new(0))).collect();
            file_counters.push((i, counters.clone()));
            specs.push(Spec {
                idx: i,
                uris: file_uris,
                out,
                options: opts,
                counters,
                chunk,
                adopted: Arc::new(parking_lot::Mutex::new(None)),
            });
        }

        let agg = Counters::new(0, 0);
        let worker_epoch = next_worker_epoch();
        tokio::spawn(async move {
            let mut ad = agg.to_active(
                worker_epoch,
                Vec::new(),
                Arc::new(parking_lot::Mutex::new(None)),
            );
            ad.cancel_token = parent;
            ad.metalink_files = file_counters.clone();
            active.write().await.insert(gid.clone(), ad);

            let futs = specs.into_iter().map(|spec| {
                let gl = global_limiter.clone();
                let tl = task_speed_limiter.clone();
                let dir = dir.clone();
                async move {
                    let c = spec.counters;
                    let r = http::run_http_download_multi(
                        &spec.uris,
                        &dir,
                        &spec.out,
                        &spec.options,
                        c.total,
                        c.completed,
                        c.speed,
                        c.connections,
                        c.cancel_token,
                        gl,
                        tl,
                        spec.chunk,
                        spec.adopted,
                    )
                    .await;
                    (spec.idx, spec.out, r)
                }
            });
            let results = futures_util::future::join_all(futs).await;

            metalink_finish(
                &tasks,
                &active,
                &events,
                &gid,
                worker_epoch,
                file_counters,
                results,
            )
            .await;
        });
    }

    fn spawn_ed2k_download(&self, task: &DownloadTask) {
        let gid = task.gid.clone();
        let uri = task.uris.first().cloned().unwrap_or_default();
        let dir = task.dir.clone();
        let events = self.events.clone();
        let tasks = self.tasks.clone();
        let active = self.active_downloads.clone();
        let options = self.options.clone();

        let counters = Counters::new(task.total_length, 0);
        let worker_epoch = next_worker_epoch();
        tokio::spawn(async move {
            active.write().await.insert(
                gid.clone(),
                counters.to_active(
                    worker_epoch,
                    Vec::new(),
                    Arc::new(parking_lot::Mutex::new(None)),
                ),
            );

            let file_link = super::ed2k::parse_ed2k_link(&uri);
            let opts_guard = options.read().await;
            let ed2k_servers = opts_guard.ed2k_servers();
            let ed2k_port = opts_guard.ed2k_port();
            drop(opts_guard);

            let c = counters.clone();
            let download_result = match file_link {
                Ok(link) => {
                    super::ed2k::run_ed2k_download(
                        &link,
                        &dir,
                        ed2k_servers,
                        ed2k_port,
                        c.total,
                        c.completed,
                        c.speed,
                        c.connections,
                        c.cancel_token,
                    )
                    .await
                }
                Err(e) => Err(e),
            };

            finish_task(
                &tasks,
                &active,
                &events,
                &gid,
                worker_epoch,
                "ed2k",
                &counters,
                download_result,
                |_| {},
                |task, _| task.total_length,
                |_, e| classify_error(e, "ed2k"),
            )
            .await;
        });
    }

    fn spawn_media_download(&self, task: &DownloadTask, merged_options: Map<String, Value>) {
        let gid = task.gid.clone();
        let uri = task.uris.first().cloned().unwrap_or_default();
        let dir = task.dir.clone();
        let out = task.out.clone();
        let events = self.events.clone();
        let tasks = self.tasks.clone();
        let active = self.active_downloads.clone();
        let global_limiter = self.global_speed_limiter.clone();

        let counters = Counters::new(0, 1);

        // Watch channel: yt-dlp sends the resolved output path before
        // download starts so the task name updates in real time
        let (dest_tx, mut dest_rx) = tokio::sync::watch::channel(String::new());

        // Spawn a lightweight watcher that updates files[0].path whenever
        // yt-dlp reports a new destination (before_dl / Destination: lines)
        let tasks_name = tasks.clone();
        let gid_name = gid.clone();
        tokio::spawn(async move {
            while dest_rx.changed().await.is_ok() {
                let dest = dest_rx.borrow().clone();
                if dest.is_empty() {
                    continue;
                }
                let mut guard = tasks_name.write().await;
                if let Some(t) = guard.iter_mut().find(|t| t.gid == gid_name) {
                    if let Some(f) = t.files.get_mut(0) {
                        f.path = dest;
                    }
                }
            }
        });

        let worker_epoch = next_worker_epoch();
        tokio::spawn(async move {
            active.write().await.insert(
                gid.clone(),
                counters.to_active(
                    worker_epoch,
                    Vec::new(),
                    Arc::new(parking_lot::Mutex::new(None)),
                ),
            );

            // Snapshot the global limit at launch time for this yt-dlp child
            // Runtime max-overall-download-limit changes do not reconfigure
            // already-running media subprocesses
            let global_rate_limit = global_limiter.limit_bps();

            let c = counters.clone();
            let download_result = media::run_media_download(
                &uri,
                &dir,
                &out,
                &merged_options,
                global_rate_limit,
                c.total,
                c.completed,
                c.speed,
                c.connections,
                c.cancel_token,
                dest_tx,
            )
            .await;

            finish_task(
                &tasks,
                &active,
                &events,
                &gid,
                worker_epoch,
                "media",
                &counters,
                download_result,
                |_| {},
                |task, path| {
                    if let Ok(metadata) = std::fs::metadata(path) {
                        let file_size = metadata.len();
                        task.completed_length = file_size;
                        if task.total_length == 0 {
                            task.total_length = file_size;
                        }
                    }
                    task.completed_length
                },
                |_, e| classify_error(e, "media"),
            )
            .await;
        });
    }

    fn spawn_m3u8_download(&self, task: &DownloadTask, merged_options: Map<String, Value>) {
        let gid = task.gid.clone();
        let uri = task.uris.first().cloned().unwrap_or_default();
        let dir = task.dir.clone();
        let out = task.out.clone();
        let events = self.events.clone();
        let tasks = self.tasks.clone();
        let active = self.active_downloads.clone();

        let per_task_limit = merged_options
            .get("max-download-limit")
            .map(parse_speed_limit)
            .unwrap_or(0);
        let task_speed_limiter = Arc::new(SpeedLimiter::new(per_task_limit));
        let global_limiter = self.global_speed_limiter.clone();

        let counters = Counters::new(0, 0);
        let worker_epoch = next_worker_epoch();
        tokio::spawn(async move {
            active.write().await.insert(
                gid.clone(),
                counters.to_active(
                    worker_epoch,
                    Vec::new(),
                    Arc::new(parking_lot::Mutex::new(None)),
                ),
            );

            let c = counters.clone();
            let download_result = super::m3u8::run_m3u8_download(
                &uri,
                &dir,
                &out,
                &merged_options,
                c.total,
                c.completed,
                c.speed,
                c.connections,
                c.cancel_token,
                global_limiter,
                task_speed_limiter,
            )
            .await;

            finish_task(
                &tasks,
                &active,
                &events,
                &gid,
                worker_epoch,
                "m3u8",
                &counters,
                download_result,
                |_| {},
                |task, _| task.total_length,
                |_, e| classify_error(e, "m3u8"),
            )
            .await;
        });
    }

    fn spawn_ftp_download(&self, task: &DownloadTask, merged_options: Map<String, Value>) {
        let gid = task.gid.clone();
        let uri = task.uris.first().cloned().unwrap_or_default();
        let dir = task.dir.clone();
        let out = task.out.clone();
        let events = self.events.clone();
        let tasks = self.tasks.clone();
        let active = self.active_downloads.clone();

        let per_task_limit = merged_options
            .get("max-download-limit")
            .map(parse_speed_limit)
            .unwrap_or(0);
        let task_speed_limiter = Arc::new(SpeedLimiter::new(per_task_limit));
        let global_limiter = self.global_speed_limiter.clone();

        let counters = Counters::new(0, 1);
        let worker_epoch = next_worker_epoch();
        tokio::spawn(async move {
            active.write().await.insert(
                gid.clone(),
                counters.to_active(
                    worker_epoch,
                    Vec::new(),
                    Arc::new(parking_lot::Mutex::new(None)),
                ),
            );

            let c = counters.clone();
            let download_result = super::ftp::run_ftp_download(
                &uri,
                &dir,
                &out,
                &merged_options,
                c.total,
                c.completed,
                c.speed,
                c.connections,
                c.cancel_token,
                global_limiter,
                task_speed_limiter,
            )
            .await;

            finish_task(
                &tasks,
                &active,
                &events,
                &gid,
                worker_epoch,
                "ftp",
                &counters,
                download_result,
                |_| {},
                |task, _| task.total_length,
                |_, e| classify_error(e, "ftp"),
            )
            .await;
        });
    }

    /// Update progress for all active downloads
    /// Also starts waiting tasks if slots are available
    pub async fn update_progress(&self) {
        // Cap stopped task history once per tick to avoid long-uptime growth
        self.enforce_result_cap().await;

        {
            let tasks_ro = self.tasks.read().await;
            let any_work = tasks_ro.iter().any(|t| {
                matches!(
                    t.status,
                    TaskStatus::Active | TaskStatus::Waiting | TaskStatus::Scheduled
                )
            });
            if !any_work {
                return;
            }
        }

        {
            // Snapshot the raw global seeding options before tasks.write() to
            // avoid cross-lock awaits in the tick. Kept raw (not collapsed to
            // effective values) so each task can override them from its own
            // options below, falling back to these globals when unset
            let (g_manual, g_seed_time, g_seed_ratio, bt_create_subfolder_default) = {
                let opts = self.options.read().await;
                // Capture the global default so per-task missing values fall
                // back to it instead of hard-coded `true`, which previously
                // ignored a user-configured `bt-create-subfolder=false`
                let csub_default = opts.get_bool("bt-create-subfolder").unwrap_or(true);
                (
                    opts.keep_seeding(),
                    opts.seed_time(),
                    opts.seed_ratio(),
                    csub_default,
                )
            };

            let active_torrent_gids = {
                let active = self.active_downloads.read().await;
                let mut tasks = self.tasks.write().await;
                let mut active_torrent_gids = Vec::new();

                for task in tasks.iter_mut() {
                    if task.status != TaskStatus::Active {
                        continue;
                    }
                    if task.kind == TaskKind::Torrent {
                        active_torrent_gids.push(task.gid.clone());
                    }
                    if let Some(ad) = active.get(&task.gid) {
                        if task.kind == TaskKind::Metalink {
                            for (idx, c) in &ad.metalink_files {
                                if let Some(f) = task.files.get_mut(*idx) {
                                    f.completed_length =
                                        c.completed.load(Ordering::Relaxed).to_string();
                                    let t = c.total.load(Ordering::Relaxed);
                                    if t > 0 {
                                        f.length = t.to_string();
                                    }
                                }
                            }
                            task.download_speed = ad
                                .metalink_files
                                .iter()
                                .map(|(_, c)| c.speed.load(Ordering::Relaxed))
                                .sum();
                            task.connections = ad
                                .metalink_files
                                .iter()
                                .map(|(_, c)| c.connections.load(Ordering::Relaxed))
                                .sum();
                            metalink_rollup_totals(task);
                            continue;
                        }
                        task.total_length = ad.total.load(Ordering::Relaxed);
                        task.completed_length = ad.completed.load(Ordering::Relaxed);
                        task.download_speed = ad.speed.load(Ordering::Relaxed);
                        task.connections = ad.connections.load(Ordering::Relaxed);

                        // Sync a Content-Disposition filename into both display path fields
                        if let Some(name) = ad.adopted_filename.lock().clone() {
                            if !name.is_empty() && task.out != name {
                                task.out = name.clone();
                                if let Some(f) = task.files.first_mut() {
                                    f.path = format!("{}/{}", task.dir, name);
                                }
                            }
                        }

                        // Split chunk progress only when multiple HTTP connections are active
                        let conns = ad.connections.load(Ordering::Relaxed);
                        if !ad.chunk_completed.is_empty() && task.total_length > 0 && conns > 1 {
                            task.chunk_progress =
                                chunk_progress(&ad.chunk_completed, task.total_length);
                        } else {
                            task.chunk_progress.clear();
                        }

                        if let Some(f) = task.files.first_mut() {
                            f.length = task.total_length.to_string();
                            f.completed_length = task.completed_length.to_string();
                            // Resolve raw URLs to disk paths
                            if looks_like_url(&f.path) {
                                let filename = if !task.out.is_empty() {
                                    task.out.clone()
                                } else if let Some(uri) = task.uris.first() {
                                    let name = http::infer_filename_from_uri(uri);
                                    format!("{name}.part")
                                } else {
                                    String::new()
                                };
                                if !filename.is_empty() {
                                    let display =
                                        filename.strip_suffix(".part").unwrap_or(&filename);
                                    f.path = format!("{}/{}", task.dir, display);
                                }
                            }
                        }
                    }
                }

                active_torrent_gids
            };

            // Snapshot torrent stats before task mutations to keep lock scope small
            let torrent_stats_by_gid: HashMap<String, torrent::TorrentStats> =
                if active_torrent_gids.is_empty() {
                    HashMap::new()
                } else {
                    let te_guard = self.torrent_engine.read().await;
                    let tid_guard = self.torrent_ids.read().await;
                    if let Some(ref te) = *te_guard {
                        active_torrent_gids
                            .into_iter()
                            .filter_map(|gid| {
                                let tid = *tid_guard.get(&gid)?;
                                te.get_torrent_stats(tid).map(|stats| (gid, stats))
                            })
                            .collect()
                    } else {
                        HashMap::new()
                    }
                };

            if !torrent_stats_by_gid.is_empty() {
                let mut tasks = self.tasks.write().await;
                for task in tasks.iter_mut() {
                    if task.kind != TaskKind::Torrent || task.status != TaskStatus::Active {
                        continue;
                    }
                    if let Some(stats) = torrent_stats_by_gid.get(&task.gid) {
                        task.total_length = stats.total_bytes;
                        task.completed_length = stats.downloaded_bytes;
                        task.upload_length = stats.uploaded_bytes;
                        task.download_speed = stats.download_speed;
                        task.upload_speed = stats.upload_speed;
                        task.connections = stats.num_peers;
                        task.num_seeders = stats.num_seeders;

                        let num_pieces = stats
                            .metadata
                            .as_ref()
                            .map(|m| m.num_pieces)
                            .unwrap_or(task.num_pieces);
                        sync_peer_infos(&mut task.peers, &stats.peers, num_pieces);

                        // Surface .torrent metadata once parsed, immediate for files and delayed for magnets
                        if let Some(ref meta) = stats.metadata {
                            task.piece_length = meta.piece_length;
                            task.num_pieces = meta.num_pieces;
                            if task.bt_comment.is_none() {
                                task.bt_comment = meta.comment.clone();
                            }
                            if task.bt_creation_date.is_none() {
                                task.bt_creation_date = meta.creation_date;
                            }
                            if task.bt_announce_list.is_empty() {
                                task.bt_announce_list = meta.announce_list.clone();
                            }
                        }

                        if task.bt_name.is_none() {
                            if let Some(ref name) = stats.name {
                                task.bt_name = Some(name.clone());
                            }
                        }

                        // Populate file list from torrent metadata
                        if let Some(ref file_details) = stats.file_details {
                            let torrent_name = task.bt_name.as_deref().unwrap_or("");
                            let create_subfolder = task
                                .options
                                .get("bt-create-subfolder")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(bt_create_subfolder_default);
                            let base_dir = if let Some(resolved_root) =
                                stats.resolved_root.as_ref().filter(|s| !s.is_empty())
                            {
                                resolved_root.clone()
                            } else if torrent_name.is_empty()
                                || stats.single_file_mode
                                || !create_subfolder
                            {
                                task.dir.clone()
                            } else {
                                // Multi-file torrents store files inside the torrent folder
                                format!("{}/{}", task.dir, torrent_name)
                            };

                            // Determine selected files as zero-based indices
                            let selected_indices: Option<std::collections::HashSet<usize>> = task
                                .options
                                .get("select-file")
                                .and_then(|v| v.as_str())
                                .and_then(|raw| {
                                    let raw = raw.trim();
                                    if raw.is_empty() {
                                        return None;
                                    }
                                    let set: std::collections::HashSet<usize> = raw
                                        .split(',')
                                        .filter_map(|s| s.trim().parse::<usize>().ok())
                                        .filter(|&i| i >= 1)
                                        .map(|i| i - 1) // 1-based to 0-based
                                        .collect();
                                    if set.is_empty() {
                                        None
                                    } else {
                                        Some(set)
                                    }
                                });

                            let (selected_total, selected_completed) = sync_torrent_files(
                                &mut task.files,
                                file_details,
                                &stats.file_progress,
                                &base_dir,
                                selected_indices.as_ref(),
                            );

                            // Override totals with selected-only sums
                            if selected_indices.is_some() {
                                task.total_length = selected_total;
                                task.completed_length = selected_completed;
                            }
                        } else if task.files.is_empty() {
                            // Fallback while metadata is unavailable
                            if let Some(ref name) = stats.name {
                                task.files = vec![DownloadFile {
                                    index: "1".to_string(),
                                    path: format!("{}/{}", task.dir, name),
                                    length: stats.total_bytes.to_string(),
                                    completed_length: stats.downloaded_bytes.to_string(),
                                    selected: "true".to_string(),
                                    uris: Vec::new(),
                                }];
                            }
                        } else {
                            // Update progress for the existing single-entry fallback
                            if let Some(f) = task.files.first_mut() {
                                f.length = stats.total_bytes.to_string();
                                f.completed_length = stats.downloaded_bytes.to_string();
                            }
                        }

                        // Per-task seeding goals override the globals; unset keys
                        // fall back to the global snapshot
                        let (keep, seed_time_minutes, seed_ratio) =
                            resolve_seed_goal(&task.options, g_manual, g_seed_time, g_seed_ratio);

                        if stats.is_finished && !task.seeder {
                            if keep {
                                // Mark as seeder while keeping Active so uploads continue
                                task.seeder = true;
                                task.seeding_since = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_millis()
                                    as u64;
                                task.download_speed = 0;
                                self.events.send(EngineEvent::BtDownloadComplete {
                                    gid: task.gid.clone(),
                                });
                            } else {
                                task.status = TaskStatus::Complete;
                                self.events.send(EngineEvent::BtDownloadComplete {
                                    gid: task.gid.clone(),
                                });
                                self.events.send(EngineEvent::DownloadComplete {
                                    gid: task.gid.clone(),
                                });
                            }
                        }

                        // Check seed time and seed ratio limits
                        if task.seeder && task.seeding_since > 0 {
                            let mut should_stop = false;

                            // Check seed time limit
                            let seed_time_ms = seed_time_minutes * 60 * 1000;
                            if seed_time_ms > 0 {
                                let now = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_millis() as u64;
                                if now >= task.seeding_since
                                    && now - task.seeding_since >= seed_time_ms
                                {
                                    should_stop = true;
                                }
                            }

                            // Check seed ratio limit
                            if !should_stop && seed_ratio > 0.0 && task.total_length > 0 {
                                let current_ratio =
                                    task.upload_length as f64 / task.total_length as f64;
                                if current_ratio >= seed_ratio {
                                    should_stop = true;
                                }
                            }

                            if should_stop {
                                task.seeder = false;
                                task.seeding_since = 0;
                                task.status = TaskStatus::Complete;
                                self.events.send(EngineEvent::DownloadComplete {
                                    gid: task.gid.clone(),
                                });
                            }
                        }
                    }
                }
            }
        }
        // Promote due scheduled tasks, then enforce the strict-priority queue
        self.ensure_active_magnet_resolvers().await;
        self.check_scheduled_tasks().await;
        self.reconcile_active_set().await;
    }

    async fn ensure_active_magnet_resolvers(&self) {
        let jobs = {
            // Acquire in the same order as remove() (torrent_ids -> pending_magnets -> tasks)
            // to avoid a deadlock where remove() holds torrent_ids.write() while waiting
            // for tasks.write() and we hold tasks.read() while waiting for torrent_ids.read()
            let torrent_ids = self.torrent_ids.read().await;
            let pending = self.pending_magnets.read().await;
            let tasks = self.tasks.read().await;
            let options = self.options.read().await;

            tasks
                .iter()
                .filter(|task| {
                    task.kind == TaskKind::Torrent
                        && task.status == TaskStatus::Active
                        && !torrent_ids.contains_key(&task.gid)
                        && !pending.contains(&task.gid)
                })
                .filter_map(|task| {
                    let uri = task
                        .uris
                        .iter()
                        .find(|uri| torrent::is_magnet_uri(uri))?
                        .clone();
                    Some((
                        task.gid.clone(),
                        uri,
                        options.merge_task_options(&task.options),
                    ))
                })
                .collect::<Vec<_>>()
        };

        for (gid, uri, options) in jobs {
            self.spawn_magnet_metadata_resolver(gid, uri, options).await;
        }
    }

    pub async fn pause(&self, gid: &str) -> Result<(), String> {
        // Cancel active HTTP download
        {
            let active = self.active_downloads.read().await;
            if let Some(ad) = active.get(gid) {
                ad.cancel_token.cancel();
            }
        }

        // Pause torrent
        {
            let tid_guard = self.torrent_ids.read().await;
            if let Some(&tid) = tid_guard.get(gid) {
                let te_guard = self.torrent_engine.read().await;
                if let Some(ref te) = *te_guard {
                    te.pause(tid).await.ok();
                }
            }
        }

        let mut tasks = self.tasks.write().await;
        if let Some(task) = tasks.iter_mut().find(|t| t.gid == gid) {
            if task.status == TaskStatus::Active || task.status == TaskStatus::Waiting {
                tracing::info!("[task:{}] Paused (was {:?})", gid, task.status);
                task.status = TaskStatus::Paused;
                task.download_speed = 0;
                task.upload_speed = 0;
                self.events.send(EngineEvent::DownloadPause {
                    gid: gid.to_string(),
                });
                return Ok(());
            }
        }
        Err(format!("Task {} not found or not active", gid))
    }

    pub async fn unpause(&self, gid: &str) -> Result<(), String> {
        tracing::info!("[task:{}] Resuming", gid);
        let is_torrent;
        {
            let mut tasks = self.tasks.write().await;
            let task = tasks
                .iter_mut()
                .find(|t| t.gid == gid)
                .ok_or_else(|| format!("Task {} not found or not paused", gid))?;
            if task.status != TaskStatus::Paused && task.status != TaskStatus::Error {
                return Err(format!("Task {} not found or not paused", gid));
            }
            task.error_code = None;
            task.error_message = None;
            is_torrent = task.kind == TaskKind::Torrent;
            if is_torrent {
                task.status = TaskStatus::Active;
            } else {
                task.status = TaskStatus::Waiting;
            }
        }

        // Resume torrent in engine
        if is_torrent {
            let tid_guard = self.torrent_ids.read().await;
            if let Some(&tid) = tid_guard.get(gid) {
                let te_guard = self.torrent_engine.read().await;
                if let Some(ref te) = *te_guard {
                    te.unpause(tid).await.ok();
                }
            } else {
                self.ensure_active_magnet_resolvers().await;
            }
        } else {
            self.try_start_next().await;
        }

        self.send_download_start(gid);
        Ok(())
    }

    /// Promote scheduled tasks whose start time has arrived
    async fn check_scheduled_tasks(&self) {
        // ponytail: 5-min tolerance separates "came due while running" from
        // "overdue because we were off"; tune if wake-from-sleep feels wrong
        const GRACE_SECS: u64 = 300;
        let now = crate::engine::util::now_secs();
        let mut tasks = self.tasks.write().await;
        for task in tasks.iter_mut() {
            if task.status != TaskStatus::Scheduled || task.schedule_missed {
                continue;
            }
            let Some(ts) = task.start_at else { continue };
            if now < ts {
                continue;
            }
            if now - ts <= GRACE_SECS {
                tracing::info!(
                    "[task:{}] Scheduled task now due, promoting to Waiting (now={}, start_at={}, elapsed={}s)",
                    task.gid,
                    now,
                    ts,
                    now - ts
                );
                task.status = TaskStatus::Waiting;
                task.start_at = None;
            } else {
                tracing::warn!(
                    "[task:{}] Scheduled task missed (now={}, start_at={}, overdue by {}s)",
                    task.gid,
                    now,
                    ts,
                    now - ts
                );
                task.schedule_missed = true;
            }
        }
    }

    pub async fn set_task_schedule(&self, gid: &str, start_at: u64) -> Result<(), String> {
        let now = crate::engine::util::now_secs();
        tracing::info!(
            "[task:{}] set_task_schedule: start_at={}, now={}, delay={}s",
            gid,
            start_at,
            now,
            start_at.saturating_sub(now)
        );
        {
            let active = self.active_downloads.read().await;
            if let Some(ad) = active.get(gid) {
                ad.cancel_token.cancel();
            }
        }
        {
            let mut tasks = self.tasks.write().await;
            let task = tasks
                .iter_mut()
                .find(|t| t.gid == gid)
                .ok_or_else(|| format!("Task {} not found", gid))?;
            if task.kind == TaskKind::Torrent {
                return Err("Scheduling torrent tasks is not supported".to_string());
            }
            if task.status.is_stopped() {
                return Err(format!("Task {} is already finished", gid));
            }
            task.status = TaskStatus::Scheduled;
            task.start_at = Some(start_at);
            task.schedule_missed = false;
            task.download_speed = 0;
            task.upload_speed = 0;
        }
        // The freed slot
        self.reconcile_active_set().await;
        Ok(())
    }

    /// Start a scheduled task immediately + clearing its schedule
    pub async fn start_task_now(&self, gid: &str) -> Result<(), String> {
        tracing::info!(
            "[task:{}] start_task_now: manually starting scheduled task",
            gid
        );
        {
            let mut tasks = self.tasks.write().await;
            let task = tasks
                .iter_mut()
                .find(|t| t.gid == gid)
                .ok_or_else(|| format!("Task {} not found", gid))?;
            if task.status != TaskStatus::Scheduled {
                return Err(format!("Task {} is not scheduled", gid));
            }
            task.status = TaskStatus::Waiting;
            task.start_at = None;
            task.schedule_missed = false;
        }
        self.reconcile_active_set().await;
        self.send_download_start(gid);
        Ok(())
    }

    /// Move one or more tasks
    pub async fn move_tasks(
        &self,
        gids: &[String],
        target_gid: &str,
        after: bool,
    ) -> Result<(), String> {
        {
            let mut guard = self.tasks.write().await;
            let move_set: std::collections::HashSet<&str> =
                gids.iter().map(|s| s.as_str()).collect();
            let mut moved = Vec::new();
            let mut remaining = Vec::new();
            for t in std::mem::take(&mut *guard) {
                if move_set.contains(t.gid.as_str()) {
                    moved.push(t);
                } else {
                    remaining.push(t);
                }
            }
            if moved.is_empty() {
                *guard = remaining;
                return Err("no matching tasks to move".to_string());
            }
            let insert_at = remaining
                .iter()
                .position(|t| t.gid == target_gid)
                .map(|p| if after { p + 1 } else { p })
                .unwrap_or(remaining.len());
            let mut result = Vec::with_capacity(remaining.len() + moved.len());
            result.extend(remaining.drain(..insert_at));
            result.extend(moved);
            result.extend(remaining);
            *guard = result;
        }
        self.reconcile_active_set().await;
        Ok(())
    }

    /// Replace an HTTP task's credentials with freshly imported ones and
    /// resume it. Called from the Cloudflare-recovery flow after the user
    /// solves a challenge in their browser. Either `cookie` or
    /// `user_agent` may be empty to leave that field unchanged
    pub async fn retry_with_cookies(
        &self,
        gid: &str,
        cookie: Option<String>,
        user_agent: Option<String>,
    ) -> Result<(), String> {
        tracing::info!("[task:{}] Retrying with imported cookies", gid);
        let was_active;
        {
            let mut tasks = self.tasks.write().await;
            let task = tasks
                .iter_mut()
                .find(|t| t.gid == gid)
                .ok_or_else(|| format!("Task {} not found", gid))?;
            if task.kind != TaskKind::Http {
                return Err(format!("Task {} is not an HTTP task", gid));
            }
            if let Some(c) = cookie {
                if !c.is_empty() {
                    task.options.insert("cookie".to_string(), Value::String(c));
                }
            }
            if let Some(ua) = user_agent {
                if !ua.is_empty() {
                    task.options
                        .insert("user-agent".to_string(), Value::String(ua));
                }
            }
            // Clear stale error state
            task.error_code = None;
            task.error_message = None;
            was_active = task.status == TaskStatus::Active;
            task.status = TaskStatus::Waiting;
        }

        if was_active {
            // Cancel any in-flight worker so the new options take effect
            let active = self.active_downloads.read().await;
            if let Some(ad) = active.get(gid) {
                ad.cancel_token.cancel();
            }
        }
        self.try_start_next().await;
        self.send_download_start(gid);
        Ok(())
    }

    pub async fn remove(&self, gid: &str) -> Result<(), String> {
        tracing::info!("[task:{}] Removing", gid);
        // Cancel any active download
        {
            let active = self.active_downloads.read().await;
            if let Some(ad) = active.get(gid) {
                ad.cancel_token.cancel();
            }
        }

        {
            let tid_guard = self.torrent_ids.read().await;
            if let Some(&tid) = tid_guard.get(gid) {
                let te_guard = self.torrent_engine.read().await;
                if let Some(ref te) = *te_guard {
                    te.remove(tid, false).await.ok();
                }
            }
        }
        self.torrent_ids.write().await.remove(gid);
        self.pending_magnets.write().await.remove(gid);

        let mut tasks = self.tasks.write().await;
        if let Some(task) = tasks.iter_mut().find(|t| t.gid == gid) {
            task.status = TaskStatus::Removed;
            task.download_speed = 0;
            task.upload_speed = 0;
            self.events.send(EngineEvent::DownloadStop {
                gid: gid.to_string(),
            });
            return Ok(());
        }
        Err(format!("Task {} not found", gid))
    }

    pub async fn tell_status(&self, gid: &str, keys: &[String]) -> Result<Value, String> {
        let tasks = self.tasks.read().await;
        tasks
            .iter()
            .find(|t| t.gid == gid)
            .map(|t| t.to_rpc_status(keys))
            .ok_or_else(|| format!("GID {} not found", gid))
    }

    pub async fn tell_active(&self, keys: &[String]) -> Value {
        let tasks = self.tasks.read().await;
        let active: Vec<Value> = tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Active)
            .map(|t| t.to_rpc_status(keys))
            .collect();
        Value::Array(active)
    }

    pub async fn tell_waiting(&self, offset: i64, num: usize, keys: &[String]) -> Value {
        let tasks = self.tasks.read().await;
        let waiting: Vec<&DownloadTask> = tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Waiting || t.status == TaskStatus::Paused)
            .collect();
        let num = num.min(10_000);
        Value::Array(Self::paginate_newest_first(&waiting, offset, num, keys))
    }

    pub async fn tell_stopped(&self, offset: i64, num: usize, keys: &[String]) -> Value {
        let tasks = self.tasks.read().await;
        let stopped: Vec<&DownloadTask> = tasks.iter().filter(|t| t.status.is_stopped()).collect();
        let num = num.min(10_000);
        Value::Array(Self::paginate_newest_first(&stopped, offset, num, keys))
    }

    pub async fn tell_scheduled(&self, offset: i64, num: usize, keys: &[String]) -> Value {
        let tasks = self.tasks.read().await;
        let scheduled: Vec<&DownloadTask> = tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Scheduled)
            .collect();
        let num = num.min(10_000);
        Value::Array(Self::paginate_newest_first(&scheduled, offset, num, keys))
    }

    fn paginate_newest_first(
        items: &[&DownloadTask],
        offset: i64,
        num: usize,
        keys: &[String],
    ) -> Vec<Value> {
        let len = items.len();
        let (start, end) = if offset >= 0 {
            let end = len.saturating_sub(offset as usize);
            let start = end.saturating_sub(num);
            (start, end)
        } else {
            let back = usize::try_from(offset.unsigned_abs()).unwrap_or(usize::MAX);
            let start = len.saturating_sub(back);
            let end = start.saturating_add(num).min(len);
            (start, end)
        };
        items[start..end]
            .iter()
            .map(|t| t.to_rpc_status(keys))
            .collect()
    }

    pub async fn get_global_stat(&self) -> Value {
        let tasks = self.tasks.read().await;
        let mut num_active = 0u64;
        let mut num_waiting = 0u64;
        let mut num_stopped = 0u64;
        let mut dl_speed = 0u64;
        let mut ul_speed = 0u64;

        for task in tasks.iter() {
            match task.status {
                TaskStatus::Active => {
                    num_active += 1;
                    dl_speed += task.download_speed;
                    ul_speed += task.upload_speed;
                }
                TaskStatus::Waiting | TaskStatus::Paused | TaskStatus::Scheduled => {
                    num_waiting += 1
                }
                _ => num_stopped += 1,
            }
        }

        serde_json::json!({
            "numActive": num_active.to_string(),
            "numWaiting": num_waiting.to_string(),
            "numStopped": num_stopped.to_string(),
            "numStoppedTotal": num_stopped.to_string(),
            "downloadSpeed": dl_speed.to_string(),
            "uploadSpeed": ul_speed.to_string(),
        })
    }

    /// Read-only BitTorrent diagnostics for the `/health` panel. Returns
    /// `None` when the torrent engine is not initialized
    pub async fn bt_health_snapshot(&self) -> Option<super::torrent::BtHealthSnapshot> {
        let te_guard = self.torrent_engine.read().await;
        te_guard.as_ref().and_then(|te| te.health_snapshot())
    }

    /// Unique tracker announce URLs across active/waiting BT tasks. Used by
    /// the `/health` panel to probe per-tracker reachability without
    /// touching the live announce loop
    pub async fn list_active_tracker_urls(&self) -> Vec<String> {
        use std::collections::BTreeSet;
        let tasks = self.tasks.read().await;
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for t in tasks.iter() {
            if !matches!(
                t.status,
                TaskStatus::Active | TaskStatus::Waiting | TaskStatus::Paused
            ) {
                continue;
            }
            for tier in &t.bt_announce_list {
                for url in tier {
                    let trimmed = url.trim();
                    if !trimmed.is_empty() {
                        seen.insert(trimmed.to_string());
                    }
                }
            }
        }
        seen.into_iter().collect()
    }

    pub async fn change_position(&self, gid: &str, pos: i64, how: &str) -> Result<Value, String> {
        let mut tasks = self.tasks.write().await;
        let waiting: Vec<usize> = tasks
            .iter()
            .enumerate()
            .filter(|(_, t)| t.status == TaskStatus::Waiting || t.status == TaskStatus::Paused)
            .map(|(i, _)| i)
            .collect();

        let current_waiting_pos = waiting
            .iter()
            .position(|&idx| tasks[idx].gid == gid)
            .ok_or_else(|| format!("GID {} not in waiting queue", gid))?;

        let target_waiting_pos = match how {
            "POS_SET" => pos.max(0) as usize,
            "POS_CUR" => (current_waiting_pos as i64 + pos).max(0) as usize,
            "POS_END" => {
                if pos >= 0 {
                    waiting.len().saturating_sub(1)
                } else {
                    (waiting.len() as i64 + pos).max(0) as usize
                }
            }
            _ => return Err("Invalid position mode".to_string()),
        };

        let target_waiting_pos = target_waiting_pos.min(waiting.len().saturating_sub(1));

        if current_waiting_pos != target_waiting_pos {
            let task_idx = waiting[current_waiting_pos];
            let task = tasks.remove(task_idx);

            // Recalculate target index in the full list
            let waiting_after_remove: Vec<usize> = tasks
                .iter()
                .enumerate()
                .filter(|(_, t)| t.status == TaskStatus::Waiting || t.status == TaskStatus::Paused)
                .map(|(i, _)| i)
                .collect();

            let insert_idx = if target_waiting_pos < waiting_after_remove.len() {
                waiting_after_remove[target_waiting_pos]
            } else {
                tasks.len()
            };

            tasks.insert(insert_idx, task);
        }

        Ok(Value::Number(serde_json::Number::from(
            target_waiting_pos as u64,
        )))
    }

    /// Return GIDs of waiting/paused tasks that are in `filter`, preserving queue order
    pub async fn get_waiting_gids_in_order(
        &self,
        filter: &std::collections::HashSet<String>,
    ) -> Vec<String> {
        let tasks = self.tasks.read().await;
        tasks
            .iter()
            .filter(|t| {
                (t.status == TaskStatus::Waiting || t.status == TaskStatus::Paused)
                    && filter.contains(&t.gid)
            })
            .map(|t| t.gid.clone())
            .collect()
    }

    pub async fn change_option(&self, gid: &str, opts: Map<String, Value>) -> Result<(), String> {
        let mut tasks = self.tasks.write().await;
        if let Some(task) = tasks.iter_mut().find(|t| t.gid == gid) {
            // If seed-time is being set to 0, stop seeding immediately
            if let Some(v) = opts.get("seed-time") {
                let val = v
                    .as_u64()
                    .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                    .unwrap_or(0);
                if val == 0 && task.seeder {
                    // Only stop if seed-ratio is also 0 or absent
                    let effective_ratio = opts
                        .get("seed-ratio")
                        .and_then(|r| {
                            r.as_f64()
                                .or_else(|| r.as_str().and_then(|s| s.parse().ok()))
                        })
                        .or_else(|| {
                            task.options.get("seed-ratio").and_then(|r| {
                                r.as_f64()
                                    .or_else(|| r.as_str().and_then(|s| s.parse().ok()))
                            })
                        });
                    if effective_ratio.is_none_or(|r| r <= 0.0) {
                        task.seeder = false;
                        task.seeding_since = 0;
                        task.status = TaskStatus::Complete;
                        self.events.send(EngineEvent::DownloadComplete {
                            gid: task.gid.clone(),
                        });
                    }
                }
            }
            for (k, v) in opts {
                task.options.insert(k, v);
            }
            return Ok(());
        }
        Err(format!("GID {} not found", gid))
    }

    pub async fn change_global_option(&self, opts: Map<String, Value>) {
        // Update the global speed limiter if speed limits changed
        if let Some(v) = opts.get("max-overall-download-limit") {
            self.global_speed_limiter.set_limit(parse_speed_limit(v));
        }

        // Does this patch touch DoH config? If so, re-apply once we've merged
        // so the change lands on the next connection without a restart
        let touches_doh = opts.keys().any(|k| k.starts_with("doh-"));

        {
            let mut options = self.options.write().await;
            for (k, v) in opts {
                options.set(k, v);
            }
            if touches_doh {
                super::dns::apply_from_options(&options.global);
            }
        }
    }

    pub async fn get_option(&self, gid: &str) -> Result<Value, String> {
        let tasks = self.tasks.read().await;
        tasks
            .iter()
            .find(|t| t.gid == gid)
            .map(|t| Value::Object(t.options.clone()))
            .ok_or_else(|| format!("GID {} not found", gid))
    }

    pub async fn get_global_option(&self) -> Value {
        Value::Object(self.options.read().await.global.clone())
    }

    pub async fn get_peers(&self, gid: &str) -> Value {
        let tasks = self.tasks.read().await;
        if let Some(task) = tasks.iter().find(|t| t.gid == gid) {
            let peers: Vec<Value> = task
                .peers
                .iter()
                .map(|p| serde_json::to_value(p).unwrap_or(Value::Null))
                .collect();
            Value::Array(peers)
        } else {
            Value::Array(Vec::new())
        }
    }

    pub async fn get_uris(&self, gid: &str) -> Result<Value, String> {
        let tasks = self.tasks.read().await;
        let task = tasks
            .iter()
            .find(|t| t.gid == gid)
            .ok_or_else(|| format!("GID {} not found", gid))?;
        // Prefer URIs from first file entry, fall back to task-level uris
        let uris: Vec<Value> = match task.files.first().filter(|f| !f.uris.is_empty()) {
            Some(file) => file
                .uris
                .iter()
                .map(|u| {
                    serde_json::json!({
                        "uri": u.uri,
                        "status": u.status,
                    })
                })
                .collect(),
            None => task
                .uris
                .iter()
                .enumerate()
                .map(|(i, u)| {
                    serde_json::json!({
                        "uri": u,
                        "status": if i == 0 { "used" } else { "waiting" },
                    })
                })
                .collect(),
        };
        Ok(Value::Array(uris))
    }

    pub async fn get_files(&self, gid: &str) -> Result<Value, String> {
        let tasks = self.tasks.read().await;
        let task = tasks
            .iter()
            .find(|t| t.gid == gid)
            .ok_or_else(|| format!("GID {} not found", gid))?;
        let files: Vec<Value> = task
            .files
            .iter()
            .map(|f| serde_json::to_value(f).unwrap_or(Value::Null))
            .collect();
        Ok(Value::Array(files))
    }

    /// Snapshot of a task's downloaded files in a form suited for the upload
    /// pipeline. Returns `(local_path, relative_remote_path, size, task_kind,
    /// override_sink_id)` per file. `relative_remote_path` is `out` joined
    /// with the file's basename so directory structure on the remote mirrors
    /// the local layout for multi-file BT tasks. Returns `None` if the gid
    /// is unknown
    pub async fn files_for_upload(
        &self,
        gid: &str,
    ) -> Option<(Vec<UploadFileSnapshot>, String, Option<String>)> {
        let tasks = self.tasks.read().await;
        let task = tasks.iter().find(|t| t.gid == gid)?;
        // TaskKind serializes with rename_all = "lowercase", producing exactly
        // these protocol labels
        let kind = serde_json::to_value(task.kind)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        let override_sink_id = task
            .options
            .get("upload-sink-id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let dir = std::path::Path::new(&task.dir);
        let snapshots = task
            .files
            .iter()
            .filter(|f| f.selected != "false")
            .filter_map(|f| {
                let local = std::path::PathBuf::from(&f.path);
                let size: u64 = match f.length.parse::<u64>() {
                    Ok(n) => n,
                    Err(e) => {
                        tracing::warn!(
                            "Skipping upload entry with unparseable size: path={} length={:?} err={}",
                            f.path,
                            f.length,
                            e
                        );
                        return None;
                    }
                };
                // Relative to the task's download dir so multi-file torrents
                // preserve their internal layout when pushed to the sink.
                // Remote paths are always `/`-separated regardless of host OS
                let rel_path = local
                    .strip_prefix(dir)
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|_| {
                        local
                            .file_name()
                            .map(std::path::PathBuf::from)
                            .unwrap_or_default()
                    });
                // Reject any parent-directory segments to prevent escaping
                // the configured remote base path on the sink side
                if rel_path
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
                {
                    tracing::warn!(
                        "Skipping upload entry with parent-directory segment: path={}",
                        f.path
                    );
                    return None;
                }
                let rel = rel_path.to_string_lossy().to_string();
                let rel = if std::path::MAIN_SEPARATOR != '/' {
                    rel.replace(std::path::MAIN_SEPARATOR, "/")
                } else {
                    rel
                };
                if rel.is_empty() || rel.split('/').any(|seg| seg == "..") {
                    return None;
                }
                let category = local
                    .file_name()
                    .and_then(|s| s.to_str())
                    .and_then(super::upload::resolve_category);
                Some(UploadFileSnapshot {
                    local_path: local,
                    remote_relative: rel,
                    size,
                    category,
                })
            })
            .collect();
        Some((snapshots, kind, override_sink_id))
    }

    pub async fn get_servers(&self, gid: &str) -> Result<Value, String> {
        let tasks = self.tasks.read().await;
        let task = tasks
            .iter()
            .find(|t| t.gid == gid)
            .ok_or_else(|| format!("GID {} not found", gid))?;
        // connection state like aria2, return a minimal structure
        let servers: Vec<Value> = task
            .files
            .iter()
            .map(|f| {
                let svrs: Vec<Value> = f
                    .uris
                    .iter()
                    .map(|u| {
                        serde_json::json!({
                            "uri": u.uri,
                            "currentUri": u.uri,
                            "downloadSpeed": "0",
                        })
                    })
                    .collect();
                serde_json::json!({
                    "index": f.index,
                    "servers": svrs,
                })
            })
            .collect();
        Ok(Value::Array(servers))
    }

    pub async fn save_session(&self) -> Result<(), String> {
        let rev = self.tasks.rev();
        if self.saved_rev.load(Ordering::Relaxed) == rev {
            return Ok(());
        }
        let tasks = self.tasks.read().await;
        let rev = self.tasks.rev();
        self.session.save(&tasks)?;
        self.saved_rev.store(rev, Ordering::Relaxed);
        Ok(())
    }

    /// Hard cap for retained finished/failed/removed task records
    const MAX_STOPPED_RESULTS: usize = 1000;

    /// Evict oldest stopped tasks beyond [`Self::MAX_STOPPED_RESULTS`]
    ///
    /// Drops torrent bt-session entries first so stale `by_hash` records do not block re-add
    async fn enforce_result_cap(&self) {
        let to_evict: Vec<String> = {
            let tasks = self.tasks.read().await;
            let stopped = tasks.iter().filter(|t| t.status.is_stopped()).count();
            if stopped <= Self::MAX_STOPPED_RESULTS {
                return;
            }
            let mut excess = stopped - Self::MAX_STOPPED_RESULTS;
            let mut gids = Vec::with_capacity(excess);
            // Tasks are appended in creation order, so evict the earliest stopped entries first
            for t in tasks.iter() {
                if excess == 0 {
                    break;
                }
                if t.status.is_stopped() {
                    gids.push(t.gid.clone());
                    excess -= 1;
                }
            }
            gids
        };

        // Drop bt-session entries before removing evicted torrent tasks
        for gid in &to_evict {
            self.drop_torrent_engine_entry(gid).await;
        }

        let evict: std::collections::HashSet<&str> = to_evict.iter().map(String::as_str).collect();
        let mut tasks = self.tasks.write().await;
        tasks.retain(|t| !(t.status.is_stopped() && evict.contains(t.gid.as_str())));
    }

    pub async fn purge_download_result(&self) {
        // Collect gids of stopped torrent tasks so we can drop their bt-session
        // entries before evicting them from the task list. Without this, the
        // bt session keeps the info-hash registered in `by_hash` and a later
        // re-add of the same magnet short-circuits to `AlreadyManaged` with
        // the old finished handle (UI shows "completed" with no download)
        let stopped_torrent_gids: Vec<String> = {
            let tasks = self.tasks.read().await;
            tasks
                .iter()
                .filter(|t| t.status.is_stopped() && t.kind == TaskKind::Torrent)
                .map(|t| t.gid.clone())
                .collect()
        };
        for gid in &stopped_torrent_gids {
            self.drop_torrent_engine_entry(gid).await;
        }
        let mut tasks = self.tasks.write().await;
        tasks.retain(|t| !t.status.is_stopped());
    }

    pub async fn remove_download_result(&self, gid: &str) -> Result<(), String> {
        // Drop the bt-session entry first (keep files on disk — this is a
        // "remove from history" operation, not a payload deletion). Without
        // this, the `by_hash` map retains the info-hash and re-adding the
        // same magnet returns the stale completed torrent until restart
        let is_stopped_torrent = {
            let tasks = self.tasks.read().await;
            tasks
                .iter()
                .any(|t| t.gid == gid && t.status.is_stopped() && t.kind == TaskKind::Torrent)
        };
        if is_stopped_torrent {
            self.drop_torrent_engine_entry(gid).await;
        }
        let mut tasks = self.tasks.write().await;
        let len_before = tasks.len();
        tasks.retain(|t| !(t.gid == gid && t.status.is_stopped()));
        if tasks.len() < len_before {
            Ok(())
        } else {
            Err(format!("GID {} not found or not stopped", gid))
        }
    }

    /// Drop a torrent task's entry from the underlying bt session WITHOUT
    /// touching on-disk files, and clear the gid->torrent-id mapping. Used
    /// by `remove_download_result` / `purge_download_result` to prevent
    /// stale `by_hash` entries from blocking re-adds of the same magnet
    async fn drop_torrent_engine_entry(&self, gid: &str) {
        let tid = {
            let tid_guard = self.torrent_ids.read().await;
            tid_guard.get(gid).copied()
        };
        let Some(tid) = tid else { return };

        let te_guard = self.torrent_engine.read().await;
        if let Some(ref te) = *te_guard {
            if let Err(e) = te.remove(tid, false).await {
                tracing::warn!(
                    "[task:{}] failed to drop torrent engine entry (id={}): {}",
                    gid,
                    tid,
                    e
                );
                return;
            }
        }

        self.torrent_ids.write().await.remove(gid);
    }

    pub async fn pause_all(&self) {
        let mut torrent_gids = Vec::new();
        {
            let mut tasks = self.tasks.write().await;
            for task in tasks.iter_mut() {
                if task.status == TaskStatus::Active || task.status == TaskStatus::Waiting {
                    if task.kind == TaskKind::Torrent {
                        torrent_gids.push(task.gid.clone());
                    }
                    task.status = TaskStatus::Paused;
                    task.download_speed = 0;
                    task.upload_speed = 0;
                    self.events.send(EngineEvent::DownloadPause {
                        gid: task.gid.clone(),
                    });
                }
            }
        }
        // Cancel all active HTTP downloads
        let active = self.active_downloads.read().await;
        for ad in active.values() {
            ad.cancel_token.cancel();
        }
        drop(active);
        // Pause all active torrents
        let tid_guard = self.torrent_ids.read().await;
        let te_guard = self.torrent_engine.read().await;
        if let Some(ref te) = *te_guard {
            for gid in &torrent_gids {
                if let Some(&tid) = tid_guard.get(gid) {
                    te.pause(tid).await.ok();
                }
            }
        }
    }

    pub async fn unpause_all(&self) {
        let mut torrent_gids = Vec::new();
        {
            let mut tasks = self.tasks.write().await;
            for task in tasks.iter_mut() {
                if task.status == TaskStatus::Paused {
                    if task.kind == TaskKind::Torrent {
                        task.status = TaskStatus::Active;
                        torrent_gids.push(task.gid.clone());
                    } else {
                        task.status = TaskStatus::Waiting;
                    }
                    self.send_download_start(&task.gid);
                }
            }
        }
        // Resume torrents in engine
        let tid_guard = self.torrent_ids.read().await;
        let te_guard = self.torrent_engine.read().await;
        if let Some(ref te) = *te_guard {
            for gid in &torrent_gids {
                if let Some(&tid) = tid_guard.get(gid) {
                    te.unpause(tid).await.ok();
                }
            }
        }
        drop(te_guard);
        drop(tid_guard);
        self.try_start_next().await;
    }

    /// Resolve a GID prefix to the full 16-char GID.
    /// Accepts full GIDs as-is, or unique prefixes (minimum 4 chars)
    pub async fn resolve_gid(&self, prefix: &str) -> Result<String, String> {
        let tasks = self.tasks.read().await;

        // Exact match first
        if tasks.iter().any(|t| t.gid == prefix) {
            return Ok(prefix.to_string());
        }

        // Prefix match (require at least 4 chars to avoid excessive ambiguity)
        if prefix.len() < 4 {
            return Err(format!("GID prefix too short: {prefix} (minimum 4 chars)"));
        }

        let matches: Vec<&str> = tasks
            .iter()
            .filter(|t| t.gid.starts_with(prefix))
            .map(|t| t.gid.as_str())
            .collect();

        match matches.len() {
            0 => Err(format!("Task {prefix} not found")),
            1 => Ok(matches[0].to_string()),
            _ => Err(format!(
                "Ambiguous GID prefix {prefix}, matches: {}",
                matches.join(", ")
            )),
        }
    }

    pub async fn shutdown(&self) {
        tracing::info!("Engine shutting down");
        // Cancel all active downloads
        let active = self.active_downloads.read().await;
        for ad in active.values() {
            ad.cancel_token.cancel();
        }
        drop(active);

        // Save session
        if let Err(e) = self.save_session().await {
            tracing::error!("Failed to save session on shutdown: {}", e);
        }

        // Shut down torrent engine
        let mut te_guard = self.torrent_engine.write().await;
        if let Some(mut te) = te_guard.take() {
            te.shutdown().await;
        }
    }
}

fn infer_m3u8_output_name(uri: &str) -> String {
    let path = uri.split('?').next().unwrap_or(uri);
    let path = path.split('#').next().unwrap_or(path);
    let name = path.rsplit('/').next().unwrap_or("download");

    if let Some(stem) = name
        .strip_suffix(".m3u8")
        .or_else(|| name.strip_suffix(".m3u"))
    {
        format!("{stem}.ts")
    } else if name.is_empty() {
        "download.ts".to_string()
    } else {
        format!("{name}.ts")
    }
}

fn looks_like_url(path: &str) -> bool {
    path.starts_with("http://")
        || path.starts_with("https://")
        || path.starts_with("ftp://")
        || path.starts_with("ed2k://")
}

fn is_retryable_magnet_resolution_error(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    lower.contains("failed to fetch metadata")
        || lower.contains("timed out")
        || lower.contains("timeout")
        || lower.contains("no peers")
        || lower.contains("no seeds")
}

/// Resolve a torrent's seeding goal from its per-task options, falling back to
/// the global snapshot for any key the task doesn't set. Returns
/// `(keep, seed_time_minutes, seed_ratio)` where `keep` is whether to seed on
/// completion; a `keep-seeding` override zeroes the time/ratio limits so the
/// torrent seeds until manually stopped.
fn resolve_seed_goal(
    opts: &Map<String, Value>,
    g_manual: bool,
    g_seed_time: u64,
    g_seed_ratio: f64,
) -> (bool, u64, f64) {
    let manual = opts
        .get("keep-seeding")
        .and_then(|v| v.as_bool())
        .unwrap_or(g_manual);
    let seed_time = opts
        .get("seed-time")
        .and_then(|v| v.as_u64())
        .unwrap_or(g_seed_time);
    let seed_ratio = opts
        .get("seed-ratio")
        .and_then(|v| {
            v.as_f64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        })
        .unwrap_or(g_seed_ratio);
    let keep = manual || seed_time > 0 || seed_ratio > 0.0;
    let eff_time = if manual { 0 } else { seed_time };
    let eff_ratio = if manual { 0.0 } else { seed_ratio };
    (keep, eff_time, eff_ratio)
}

fn sync_peer_infos(target: &mut Vec<PeerInfo>, peers: &[torrent::PeerSnapshot], num_pieces: u32) {
    *target = peers
        .iter()
        .map(|p| {
            // one small number instead of raw bitfield hex — the hex payload
            // was up to pieces/4 chars per peer on every detail poll tick
            let percent = if p.seeder {
                100
            } else if num_pieces > 0 {
                // real piece count as denominator: padded bitfield bits
                // under-read small torrents (8 of 9 pieces read 50, not 88).
                // Count only bits below num_pieces — non-conformant peers can
                // set trailing padding bits and inflate the count otherwise
                let full_bytes = (num_pieces / 8) as usize;
                let mut ones: u64 = p.bitfield[..full_bytes.min(p.bitfield.len())]
                    .iter()
                    .map(|b| u64::from(b.count_ones()))
                    .sum();
                let trailing_bits = num_pieces % 8;
                if trailing_bits > 0 {
                    if let Some(&b) = p.bitfield.get(full_bytes) {
                        let mask = 0xffu8 << (8 - trailing_bits);
                        ones += u64::from((b & mask).count_ones());
                    }
                }
                (ones * 100 / num_pieces as u64) as u8
            } else {
                // magnet before metadata: piece count unknown yet
                0
            };
            PeerInfo {
                ip: p.addr.ip().to_string(),
                port: p.addr.port().to_string(),
                percent,
                am_choking: p.am_choking.to_string(),
                peer_choking: p.peer_choking.to_string(),
                seeder: p.seeder.to_string(),
            }
        })
        .collect();
}

fn sync_torrent_files(
    target: &mut Vec<DownloadFile>,
    file_details: &[torrent::TorrentFileInfo],
    file_progress: &[u64],
    base_dir: &str,
    selected_indices: Option<&std::collections::HashSet<usize>>,
) -> (u64, u64) {
    let mut selected_total = 0;
    let mut selected_completed = 0;
    *target = file_details
        .iter()
        .map(|fd| {
            let completed = file_progress.get(fd.index).copied().unwrap_or(0);
            let is_selected = selected_indices.is_none_or(|set| set.contains(&fd.index));
            if is_selected {
                selected_total += fd.length;
                selected_completed += completed;
            }
            DownloadFile {
                // 1-based index for aria2/RPC compatibility
                index: (fd.index + 1).to_string(),
                path: format!("{}/{}", base_dir, fd.path),
                length: fd.length.to_string(),
                completed_length: completed.to_string(),
                selected: is_selected.to_string(),
                uris: Vec::new(),
            }
        })
        .collect();

    (selected_total, selected_completed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_percent_from_bitfield() {
        let snap = torrent::PeerSnapshot {
            addr: "1.2.3.4:6881".parse().unwrap(),
            bitfield: std::sync::Arc::from([0xFFu8, 0xF0].as_slice()),
            am_choking: false,
            am_interested: false,
            peer_choking: false,
            peer_interested: false,
            seeder: false,
        };
        let peers = [snap];
        let mut target = Vec::new();

        sync_peer_infos(&mut target, &peers, 13);
        assert_eq!(target[0].percent, 92);
        assert_eq!(target[0].ip, "1.2.3.4");
        assert_eq!(target[0].port, "6881");

        sync_peer_infos(&mut target, &peers, 0);
        assert_eq!(target[0].percent, 0);

        let padded = torrent::PeerSnapshot {
            addr: "1.2.3.4:6881".parse().unwrap(),
            bitfield: std::sync::Arc::from([0xFFu8, 0x07].as_slice()),
            am_choking: false,
            am_interested: false,
            peer_choking: false,
            peer_interested: false,
            seeder: false,
        };
        sync_peer_infos(&mut target, &[padded], 13);
        assert_eq!(target[0].percent, 61);
    }

    #[test]
    fn parse_cf_host_extracts_host_from_marker() {
        assert_eq!(
            parse_cf_host("[cloudflare-challenge] host=www.spigotmc.org status=403").as_deref(),
            Some("www.spigotmc.org")
        );
    }

    #[test]
    fn parse_cf_host_returns_none_when_absent() {
        assert!(parse_cf_host("HTTP error: 403").is_none());
    }

    // -- infer_m3u8_output_name --

    #[test]
    fn m3u8_name_strips_extension() {
        assert_eq!(
            infer_m3u8_output_name("http://example.com/video.m3u8"),
            "video.ts"
        );
    }

    #[test]
    fn m3u_name_strips_extension() {
        assert_eq!(
            infer_m3u8_output_name("http://example.com/video.m3u"),
            "video.ts"
        );
    }

    #[test]
    fn m3u8_name_ignores_query_string() {
        assert_eq!(
            infer_m3u8_output_name("http://example.com/video.m3u8?token=abc"),
            "video.ts"
        );
    }

    #[test]
    fn m3u8_name_no_extension() {
        assert_eq!(
            infer_m3u8_output_name("http://example.com/video"),
            "video.ts"
        );
    }

    #[test]
    fn m3u8_name_empty_segment() {
        assert_eq!(infer_m3u8_output_name("http://example.com/"), "download.ts");
    }

    #[test]
    fn m3u8_name_bare_name() {
        assert_eq!(infer_m3u8_output_name("download"), "download.ts");
    }

    // -- looks_like_url --

    #[test]
    fn looks_like_url_protocols() {
        assert!(looks_like_url("http://example.com"));
        assert!(looks_like_url("https://example.com"));
        assert!(looks_like_url("ftp://files.example.com"));
        assert!(looks_like_url("ed2k://|file|test|100|hash|/"));
    }

    #[test]
    fn looks_like_url_paths() {
        assert!(!looks_like_url("/path/to/file"));
        assert!(!looks_like_url("file.txt"));
        assert!(!looks_like_url(""));
    }

    #[test]
    fn per_task_seed_goal_overrides_global() {
        // Globals off: a per-task seed-time still starts and bounds seeding
        let mut opts = Map::new();
        opts.insert("seed-time".into(), serde_json::json!(30));
        let (keep, time, ratio) = resolve_seed_goal(&opts, false, 0, 0.0);
        assert!(keep);
        assert_eq!(time, 30);
        assert_eq!(ratio, 0.0);

        // Unset per-task keys fall back to the globals
        let (keep, time, ratio) = resolve_seed_goal(&Map::new(), false, 10, 1.5);
        assert!(keep);
        assert_eq!(time, 10);
        assert_eq!(ratio, 1.5);

        // keep-seeding zeroes the limits so seeding runs until stopped
        let mut opts = Map::new();
        opts.insert("keep-seeding".into(), serde_json::json!(true));
        let (keep, time, ratio) = resolve_seed_goal(&opts, false, 99, 9.0);
        assert!(keep);
        assert_eq!(time, 0);
        assert_eq!(ratio, 0.0);

        // string ratio (config-set form) still parses
        let mut opts = Map::new();
        opts.insert("seed-ratio".into(), serde_json::json!("2.0"));
        let (keep, _t, ratio) = resolve_seed_goal(&opts, false, 0, 0.0);
        assert!(keep);
        assert_eq!(ratio, 2.0);

        // all off -> no seeding
        let (keep, _t, _r) = resolve_seed_goal(&Map::new(), false, 0, 0.0);
        assert!(!keep);
    }

    // -- TaskManager async query tests --

    use tokio::sync::RwLock;

    /// Build a TaskManager with pre-loaded tasks, no torrent engine needed
    fn make_test_manager(tasks: Vec<DownloadTask>) -> TaskManager {
        let dir = tempfile::TempDir::new().unwrap();
        let session = SessionManager::new(dir.path());
        let options = EngineOptions::from_config(&Map::new(), &Map::new());
        let events = EventBroadcaster::new(16);
        let global_speed_limiter = Arc::new(SpeedLimiter::new(0));
        let cookie_store = Arc::new(CookieStore::new(dir.path()));

        TaskManager {
            tasks: Arc::new(RevLock::new(tasks)),
            saved_rev: AtomicU64::new(u64::MAX),
            active_downloads: Arc::new(RwLock::new(HashMap::new())),
            torrent_ids: Arc::new(RwLock::new(HashMap::new())),
            pending_magnets: Arc::new(RwLock::new(HashSet::new())),
            purged_hashes: Arc::new(RwLock::new(HashSet::new())),
            options: Arc::new(RwLock::new(options)),
            events,
            session,
            torrent_engine: Arc::new(RwLock::new(None)),
            global_speed_limiter,
            cookie_store,
        }
    }

    #[tokio::test]
    async fn save_session_skips_until_a_write_bumps_rev() {
        let mgr = make_test_manager(vec![make_task("g1", TaskStatus::Complete)]);

        mgr.save_session().await.unwrap();
        let rev = mgr.tasks.rev();
        assert_eq!(mgr.saved_rev.load(Ordering::Relaxed), rev);

        mgr.save_session().await.unwrap();
        assert_eq!(mgr.tasks.rev(), rev, "no write must not bump rev");

        mgr.tasks
            .write()
            .await
            .push(make_task("g2", TaskStatus::Complete));
        assert!(mgr.tasks.rev() > rev, "write must bump rev");
        mgr.save_session().await.unwrap();
        assert_eq!(mgr.saved_rev.load(Ordering::Relaxed), mgr.tasks.rev());
    }

    fn make_task(gid: &str, status: TaskStatus) -> DownloadTask {
        let mut task = DownloadTask::new_http(
            gid.into(),
            vec!["http://example.com/f.bin".into()],
            "/dl".into(),
            None,
            Map::new(),
        );
        task.status = status;
        task
    }

    #[tokio::test]
    async fn move_tasks_moves_the_block_next_to_the_target() {
        // Paused tasks so the reconcile at the end never spawns a real download
        let mgr = make_test_manager(vec![
            make_task("a", TaskStatus::Paused),
            make_task("b", TaskStatus::Paused),
            make_task("c", TaskStatus::Paused),
            make_task("d", TaskStatus::Paused),
        ]);

        // Move [c, d] to just before "a"
        mgr.move_tasks(&["c".into(), "d".into()], "a", false)
            .await
            .unwrap();

        let order: Vec<String> = mgr
            .tasks
            .read()
            .await
            .iter()
            .map(|t| t.gid.clone())
            .collect();
        assert_eq!(order, vec!["c", "d", "a", "b"]);
    }

    #[tokio::test]
    async fn check_scheduled_promotes_due_and_flags_missed() {
        let now = crate::engine::util::now_secs();
        let mut due = make_task("due", TaskStatus::Scheduled);
        due.start_at = Some(now.saturating_sub(5)); // just past -> within grace, start it
        let mut missed = make_task("missed", TaskStatus::Scheduled);
        missed.start_at = Some(now.saturating_sub(10_000)); // long overdue -> flag missed
        let mut future = make_task("future", TaskStatus::Scheduled);
        future.start_at = Some(now + 10_000); // not yet due

        let mgr = make_test_manager(vec![due, missed, future]);
        mgr.check_scheduled_tasks().await;

        let tasks = mgr.tasks.read().await;
        let get = |gid: &str| tasks.iter().find(|t| t.gid == gid).unwrap();
        assert_eq!(get("due").status, TaskStatus::Waiting);
        assert!(get("due").start_at.is_none());
        assert_eq!(get("missed").status, TaskStatus::Scheduled);
        assert!(get("missed").schedule_missed);
        assert_eq!(get("future").status, TaskStatus::Scheduled);
        assert!(!get("future").schedule_missed);
    }

    #[tokio::test]
    async fn update_progress_runs_scheduler_when_only_scheduled_tasks_exist() {
        let now = crate::engine::util::now_secs();
        let mut due = make_task("due", TaskStatus::Scheduled);
        due.kind = TaskKind::Torrent;
        due.start_at = Some(now.saturating_sub(5));
        let mgr = make_test_manager(vec![due]);

        mgr.update_progress().await;

        let tasks = mgr.tasks.read().await;
        let task = tasks.iter().find(|t| t.gid == "due").unwrap();
        assert_eq!(task.status, TaskStatus::Waiting);
        assert!(task.start_at.is_none());
    }

    async fn make_test_manager_with_engine() -> (TaskManager, tempfile::TempDir) {
        let dir = tempfile::TempDir::new().unwrap();
        let mut system = Map::new();
        system.insert(
            "dir".to_string(),
            Value::String(dir.path().join("downloads").to_string_lossy().to_string()),
        );
        system.insert("bt-enable-upnp".to_string(), Value::Bool(false));
        system.insert("bt-enable-lsd".to_string(), Value::Bool(false));
        let options = EngineOptions::from_config(&system, &Map::new());
        let manager = TaskManager::new(dir.path(), options, EventBroadcaster::new(16))
            .await
            .unwrap();
        (manager, dir)
    }

    #[tokio::test]
    async fn add_magnet_task_returns_before_metadata_is_resolved() {
        let (mgr, _dir) = make_test_manager_with_engine().await;
        let uri = "magnet:?xt=urn:btih:cab507494d02ebb1178b38f2e9d7be299c86b862&dn=Metadata+Later";
        let started = std::time::Instant::now();

        let gid = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            mgr.add_magnet_task(uri, Map::new()),
        )
        .await
        .expect("add_magnet_task should not wait for metadata")
        .expect("valid magnet should create a task");

        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        assert!(!mgr.torrent_ids.read().await.contains_key(&gid));
        assert!(mgr.pending_magnets.read().await.contains(&gid));

        let tasks = mgr.tasks.read().await;
        let task = tasks.iter().find(|task| task.gid == gid).unwrap();
        assert_eq!(task.status, TaskStatus::Active);
        assert_eq!(
            task.info_hash.as_deref(),
            Some("cab507494d02ebb1178b38f2e9d7be299c86b862")
        );
        assert_eq!(task.bt_name.as_deref(), Some("Metadata Later"));
        assert_eq!(task.uris, vec![uri.to_string()]);
    }

    #[tokio::test]
    async fn shutdown_cancels_active_download_tokens() {
        let mgr = make_test_manager(Vec::new());
        let cancel_token = CancellationToken::new();
        mgr.active_downloads.write().await.insert(
            "gid1".to_string(),
            ActiveDownload {
                epoch: next_worker_epoch(),
                cancel_token: cancel_token.clone(),
                total: Arc::new(AtomicU64::new(0)),
                completed: Arc::new(AtomicU64::new(0)),
                speed: Arc::new(AtomicU64::new(0)),
                connections: Arc::new(AtomicU32::new(0)),
                chunk_completed: Vec::new(),
                adopted_filename: Arc::new(parking_lot::Mutex::new(None)),
                metalink_files: Vec::new(),
            },
        );

        mgr.shutdown().await;

        assert!(cancel_token.is_cancelled());
    }

    fn mk_metalink_file(index: &str, len: u64, done: u64) -> DownloadFile {
        DownloadFile {
            index: index.to_string(),
            path: format!("/dl/{index}.bin"),
            length: len.to_string(),
            completed_length: done.to_string(),
            selected: "true".to_string(),
            uris: vec![FileUri {
                uri: "http://mirror/".into(),
                status: "waiting".into(),
            }],
        }
    }

    async fn run_metalink_finish(
        mgr: &TaskManager,
        gid: &str,
        results: Vec<(usize, String, Result<std::path::PathBuf, String>)>,
    ) {
        let epoch = next_worker_epoch();
        let fc: Vec<(usize, Counters)> = results
            .iter()
            .map(|(i, _, _)| (*i, Counters::new(0, 1)))
            .collect();
        let mut ad = Counters::new(0, 0).to_active(
            epoch,
            Vec::new(),
            Arc::new(parking_lot::Mutex::new(None)),
        );
        ad.metalink_files = fc.clone();
        mgr.active_downloads
            .write()
            .await
            .insert(gid.to_string(), ad);
        metalink_finish(
            &mgr.tasks,
            &mgr.active_downloads,
            &mgr.events,
            gid,
            epoch,
            fc,
            results,
        )
        .await;
    }

    #[tokio::test]
    async fn metalink_parks_paused_when_a_file_fails() {
        let mut task = DownloadTask::new_metalink(
            "gm".into(),
            "/dl".into(),
            None,
            Map::new(),
            vec![
                mk_metalink_file("1", 100, 100),
                mk_metalink_file("2", 100, 0),
            ],
        );
        task.status = TaskStatus::Active;
        let mgr = make_test_manager(vec![task]);

        run_metalink_finish(
            &mgr,
            "gm",
            vec![
                (0, "a".into(), Ok(std::path::PathBuf::from("/dl/a"))),
                (1, "b".into(), Err("all mirrors failed".into())),
            ],
        )
        .await;

        let tasks = mgr.tasks.read().await;
        let t = tasks.iter().find(|t| t.gid == "gm").unwrap();
        assert_eq!(t.status, TaskStatus::Paused);
        assert_ne!(t.status, TaskStatus::Complete);
        assert!(t.error_code.is_some());
    }

    #[tokio::test]
    async fn metalink_completes_when_all_files_ok() {
        let mut task = DownloadTask::new_metalink(
            "gm".into(),
            "/dl".into(),
            None,
            Map::new(),
            vec![
                mk_metalink_file("1", 100, 100),
                mk_metalink_file("2", 100, 100),
            ],
        );
        task.status = TaskStatus::Active;
        let mgr = make_test_manager(vec![task]);

        run_metalink_finish(
            &mgr,
            "gm",
            vec![
                (0, "a".into(), Ok(std::path::PathBuf::from("/dl/a"))),
                (1, "b".into(), Ok(std::path::PathBuf::from("/dl/b"))),
            ],
        )
        .await;

        let tasks = mgr.tasks.read().await;
        let t = tasks.iter().find(|t| t.gid == "gm").unwrap();
        assert_eq!(t.status, TaskStatus::Complete);
        assert!(t.error_code.is_none());
    }

    #[tokio::test]
    async fn metalink_all_cancelled_does_not_complete() {
        let mut task = DownloadTask::new_metalink(
            "gm".into(),
            "/dl".into(),
            None,
            Map::new(),
            vec![mk_metalink_file("1", 100, 0), mk_metalink_file("2", 100, 0)],
        );
        task.status = TaskStatus::Active;
        let mgr = make_test_manager(vec![task]);

        run_metalink_finish(
            &mgr,
            "gm",
            vec![
                (0, "a".into(), Err("download cancelled".into())),
                (1, "b".into(), Err("download cancelled".into())),
            ],
        )
        .await;

        let tasks = mgr.tasks.read().await;
        let t = tasks.iter().find(|t| t.gid == "gm").unwrap();
        assert_ne!(t.status, TaskStatus::Complete);
        assert_eq!(t.status, TaskStatus::Paused);
        assert!(t.error_code.is_none());
    }

    #[tokio::test]
    async fn tell_active_returns_only_active() {
        let mgr = make_test_manager(vec![
            make_task("a1", TaskStatus::Active),
            make_task("w1", TaskStatus::Waiting),
            make_task("a2", TaskStatus::Active),
        ]);

        let result = mgr.tell_active(&[]).await;
        let arr = result.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0].get("gid").unwrap(), "a1");
        assert_eq!(arr[1].get("gid").unwrap(), "a2");
    }

    #[tokio::test]
    async fn tell_waiting_pagination() {
        let mgr = make_test_manager(vec![
            make_task("w1", TaskStatus::Waiting),
            make_task("w2", TaskStatus::Paused),
            make_task("w3", TaskStatus::Waiting),
            make_task("a1", TaskStatus::Active),
        ]);

        // offset=0, num=2 → the two NEWEST waiting/paused, in chronological order
        let result = mgr.tell_waiting(0, 2, &[]).await;
        let arr = result.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0].get("gid").unwrap(), "w2");
        assert_eq!(arr[1].get("gid").unwrap(), "w3");

        // offset=1, num=10 → skip the newest, get the rest from the start
        let result = mgr.tell_waiting(1, 10, &[]).await;
        let arr = result.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0].get("gid").unwrap(), "w1");
        assert_eq!(arr[1].get("gid").unwrap(), "w2");
    }

    #[tokio::test]
    async fn tell_waiting_newest_visible_past_cap() {
        let mut tasks = Vec::new();
        for i in 0..1005 {
            tasks.push(make_task(&format!("w{i}"), TaskStatus::Waiting));
        }
        let mgr = make_test_manager(tasks);

        let result = mgr.tell_waiting(0, 1000, &[]).await;
        let arr = result.as_array().unwrap();
        assert_eq!(arr.len(), 1000);
        assert_eq!(arr.last().unwrap().get("gid").unwrap(), "w1004");
        assert_eq!(arr.first().unwrap().get("gid").unwrap(), "w5");
    }

    #[tokio::test]
    async fn tell_waiting_negative_offset() {
        let mgr = make_test_manager(vec![
            make_task("w1", TaskStatus::Waiting),
            make_task("w2", TaskStatus::Waiting),
            make_task("w3", TaskStatus::Waiting),
        ]);

        // offset=-1 → start from last item
        let result = mgr.tell_waiting(-1, 10, &[]).await;
        let arr = result.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0].get("gid").unwrap(), "w3");
    }

    #[tokio::test]
    async fn tell_stopped_filters_correctly() {
        let mgr = make_test_manager(vec![
            make_task("c1", TaskStatus::Complete),
            make_task("e1", TaskStatus::Error),
            make_task("w1", TaskStatus::Waiting),
            make_task("r1", TaskStatus::Removed),
        ]);

        let result = mgr.tell_stopped(0, 10, &[]).await;
        let arr = result.as_array().unwrap();
        assert_eq!(arr.len(), 3);
    }

    #[tokio::test]
    async fn get_global_stat_counts() {
        let mut active = make_task("a1", TaskStatus::Active);
        active.download_speed = 1000;
        active.upload_speed = 200;

        let mgr = make_test_manager(vec![
            active,
            make_task("w1", TaskStatus::Waiting),
            make_task("w2", TaskStatus::Paused),
            make_task("c1", TaskStatus::Complete),
        ]);

        let stat = mgr.get_global_stat().await;
        assert_eq!(stat.get("numActive").unwrap(), "1");
        assert_eq!(stat.get("numWaiting").unwrap(), "2");
        assert_eq!(stat.get("numStopped").unwrap(), "1");
        assert_eq!(stat.get("downloadSpeed").unwrap(), "1000");
        assert_eq!(stat.get("uploadSpeed").unwrap(), "200");
    }

    #[tokio::test]
    async fn tell_status_found_and_not_found() {
        let mgr = make_test_manager(vec![make_task("gid1", TaskStatus::Active)]);

        let result = mgr.tell_status("gid1", &[]).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().get("gid").unwrap(), "gid1");

        let result = mgr.tell_status("nonexistent", &[]).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn change_position_set() {
        let mgr = make_test_manager(vec![
            make_task("w1", TaskStatus::Waiting),
            make_task("w2", TaskStatus::Waiting),
            make_task("w3", TaskStatus::Waiting),
        ]);

        // Move w3 to position 0 (front)
        let result = mgr.change_position("w3", 0, "POS_SET").await;
        assert!(result.is_ok());

        // Verify w3 is now first in waiting
        let waiting = mgr.tell_waiting(0, 10, &[]).await;
        let arr = waiting.as_array().unwrap();
        assert_eq!(arr[0].get("gid").unwrap(), "w3");
    }

    #[tokio::test]
    async fn change_position_cur() {
        let mgr = make_test_manager(vec![
            make_task("w1", TaskStatus::Waiting),
            make_task("w2", TaskStatus::Waiting),
            make_task("w3", TaskStatus::Waiting),
        ]);

        // Move w1 forward by 1 (relative)
        let result = mgr.change_position("w1", 1, "POS_CUR").await;
        assert!(result.is_ok());

        let waiting = mgr.tell_waiting(0, 10, &[]).await;
        let arr = waiting.as_array().unwrap();
        assert_eq!(arr[0].get("gid").unwrap(), "w2");
        assert_eq!(arr[1].get("gid").unwrap(), "w1");
    }

    #[tokio::test]
    async fn change_position_end() {
        let mgr = make_test_manager(vec![
            make_task("w1", TaskStatus::Waiting),
            make_task("w2", TaskStatus::Waiting),
            make_task("w3", TaskStatus::Waiting),
        ]);

        // Move w1 to end
        let result = mgr.change_position("w1", 0, "POS_END").await;
        assert!(result.is_ok());

        let waiting = mgr.tell_waiting(0, 10, &[]).await;
        let arr = waiting.as_array().unwrap();
        assert_eq!(arr[0].get("gid").unwrap(), "w2");
        assert_eq!(arr[1].get("gid").unwrap(), "w3");
        assert_eq!(arr[2].get("gid").unwrap(), "w1");
    }
}
