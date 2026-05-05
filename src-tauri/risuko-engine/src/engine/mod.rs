pub mod ed2k;
pub mod error_code;
pub mod events;
pub mod falloc;
pub mod ftp;
pub mod hasher;
pub mod http;
pub mod m3u8;
pub mod manager;
pub mod netrc;
pub mod options;
pub mod rpc;
pub mod rss;
pub mod session;
pub mod speed_limiter;
pub mod ssh_known_hosts;
pub mod task;
pub mod torrent;
pub mod upload;
pub mod uri_selector;
pub mod youtube;

pub use session::SESSION_FILENAME;

/// Suffix for per-chunk resume metadata sidecar file
pub const CHUNK_META_SUFFIX: &str = ".chunks";

/// Startup-only config keys (changes require an engine restart to take effect).
/// Mirrors the list maintained on the frontend in `src/shared/configKeys.ts`
pub const STARTUP_ONLY_KEYS: &[&str] = &[
    "rpc-listen-port",
    "rpc-secret",
    "listen-port",
    "dht-listen-port",
    "ed2k-port",
    "bt-max-peers-per-torrent",
    "bt-max-outstanding-per-peer",
    "bt-enable-upnp",
    "bt-upnp-lease",
    "bt-enable-lsd",
    "bt-encryption-policy",
    "bt-listen-v6",
];

use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

use crate::config::ConfigManager;
use crate::traits::{EventSink, StorageBackend};

use self::events::EventBroadcaster;
use self::manager::TaskManager;
use self::options::EngineOptions;
use self::rpc::RpcServer;
use self::upload::UploadSinkManager;

static ENGINE_INSTANCE: std::sync::LazyLock<Mutex<Option<EngineInstance>>> =
    std::sync::LazyLock::new(|| Mutex::new(None));

/// Set when the engine successfully starts; cleared on stop. Read by health checks
/// to derive uptime without coupling to TaskManager internals
static ENGINE_STARTED_AT: std::sync::Mutex<Option<Instant>> = std::sync::Mutex::new(None);

/// Snapshot of the values of `STARTUP_ONLY_KEYS` at the moment the engine last
/// started. Health checks compare this against the live config to flag drift
static STARTUP_SNAPSHOT: std::sync::Mutex<
    Option<std::collections::HashMap<String, serde_json::Value>>,
> = std::sync::Mutex::new(None);

/// Time since the engine last successfully started, or `None` if not running
pub fn engine_uptime() -> Option<Duration> {
    ENGINE_STARTED_AT
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|t| Instant::now().duration_since(*t)))
}

/// Snapshot of startup-only keys captured the last time the engine started
/// `None` until the engine has run at least once in this process
pub fn startup_snapshot() -> Option<std::collections::HashMap<String, serde_json::Value>> {
    STARTUP_SNAPSHOT.lock().ok().and_then(|g| g.clone())
}

struct EngineInstance {
    manager: Arc<TaskManager>,
    rpc_server: RpcServer,
    progress_task: Option<tokio::task::JoinHandle<()>>,
    auto_save_task: Option<tokio::task::JoinHandle<()>>,
    event_bridge_task: Option<tokio::task::JoinHandle<()>>,
}

fn is_local_rpc_host(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost" | "::1" | "[::1]")
}

pub fn should_start_embedded_engine(config: &ConfigManager) -> bool {
    let host = config
        .get_user_config()
        .get("rpc-host")
        .and_then(|v| v.as_str())
        .unwrap_or("127.0.0.1")
        .trim()
        .to_lowercase();

    is_local_rpc_host(host.as_str())
}

/// Start the engine with explicit dependencies (no Tauri required).
///
/// - `config`: the loaded ConfigManager
/// - `event_sink`: receives engine events (Tauri emitter, NAPI callback, or no-op)
/// - `storage`: persistent storage for RSS data etc
/// - `upload_sinks`: optional cloud-upload manager — when present, completed
///   downloads are forwarded to it via the event bridge
pub async fn start_engine(
    config: &ConfigManager,
    event_sink: Arc<dyn EventSink>,
    _storage: Arc<dyn StorageBackend>,
    upload_sinks: Option<Arc<UploadSinkManager>>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Check if already running
    {
        let guard = ENGINE_INSTANCE.lock().await;
        if guard.is_some() {
            log::info!("Engine already running");
            return Ok(());
        }
    }

    let config_dir = config.config_dir().to_path_buf();
    let system = config.get_system_config();
    let user = config.get_user_config();
    let options = EngineOptions::from_config(system, user);

    std::fs::create_dir_all(&config_dir)?;

    // Create the download directory if configured
    let dir = options.dir();
    if !dir.is_empty() {
        std::fs::create_dir_all(&dir).ok();
    }

    let events = EventBroadcaster::default();
    let rpc_host = options.rpc_host();
    let rpc_port = options.rpc_listen_port();
    let rpc_secret = options.rpc_secret();

    log::info!("Starting Risuko engine (in-process)");

    let manager = Arc::new(
        TaskManager::new(&config_dir, options, events.clone())
            .await
            .map_err(|e| format!("Failed to create task manager: {}", e))?,
    );

    let (rpc_shutdown_tx, mut rpc_shutdown_rx) = tokio::sync::mpsc::channel::<()>(1);

    let mut rpc_server = RpcServer::new(
        rpc_host,
        rpc_port,
        rpc_secret,
        manager.clone(),
        events.clone(),
        rpc_shutdown_tx,
    );
    rpc_server
        .start()
        .await
        .map_err(|e| format!("Failed to start RPC server: {}", e))?;

    // Start periodic progress update
    let mgr_for_progress = manager.clone();
    let progress_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
        loop {
            interval.tick().await;
            mgr_for_progress.update_progress().await;
        }
    });

    // Start periodic session auto-save
    let mgr_for_save = manager.clone();
    let auto_save_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            interval.tick().await;
            if let Err(e) = mgr_for_save.save_session().await {
                log::warn!("Auto-save session failed: {}", e);
            }
        }
    });

    // Bridge engine events to the event sink
    let sink = event_sink.clone();
    let mut event_rx = events.subscribe();
    let mgr_for_uploads = manager.clone();
    let upload_mgr = upload_sinks.clone();
    let event_bridge_task = tokio::spawn(async move {
        use events::EngineEvent;
        loop {
            match event_rx.recv().await {
                Ok(event) => {
                    let (name, gid) = match &event {
                        EngineEvent::DownloadStart { gid } => {
                            ("engine:download-start", gid.as_str())
                        }
                        EngineEvent::DownloadPause { gid } => {
                            ("engine:download-pause", gid.as_str())
                        }
                        EngineEvent::DownloadStop { gid } => ("engine:download-stop", gid.as_str()),
                        EngineEvent::DownloadComplete { gid } => {
                            ("engine:download-complete", gid.as_str())
                        }
                        EngineEvent::DownloadError { gid } => {
                            ("engine:download-error", gid.as_str())
                        }
                        EngineEvent::BtDownloadComplete { gid } => {
                            ("engine:bt-download-complete", gid.as_str())
                        }
                    };
                    let payload = serde_json::json!({ "gid": gid });
                    sink.emit(name, payload);

                    // Forward completed downloads to the upload pipeline. We
                    // dispatch on `download-complete` only — the BT-specific
                    // event fires before the final move-to-dir step on some
                    // tasks, so it's the wrong hook for cloud sync.
                    //
                    // Hand off to a detached task so a slow files_for_upload
                    // / enqueue_for_file pair can't stall the broadcast
                    // receiver and cause it to lag behind other events
                    if matches!(event, EngineEvent::DownloadComplete { .. }) {
                        if let Some(uploads) = upload_mgr.clone() {
                            let mgr = mgr_for_uploads.clone();
                            let gid_owned = gid.to_string();
                            tokio::spawn(async move {
                                let Some((files, kind, override_id)) =
                                    mgr.files_for_upload(&gid_owned).await
                                else {
                                    return;
                                };
                                for f in files {
                                    uploads
                                        .enqueue_for_file(
                                            &gid_owned,
                                            f.local_path,
                                            f.remote_relative,
                                            f.size,
                                            f.category,
                                            &kind,
                                            override_id.clone(),
                                        )
                                        .await;
                                }
                            });
                        }
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    log::warn!("Event bridge lagged by {} events", n);
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    break;
                }
            }
        }
    });

    // Clean up aria2 session file
    session::SessionManager::cleanup_legacy(&config_dir);

    let instance = EngineInstance {
        manager,
        rpc_server,
        progress_task: Some(progress_task),
        auto_save_task: Some(auto_save_task),
        event_bridge_task: Some(event_bridge_task),
    };

    *ENGINE_INSTANCE.lock().await = Some(instance);

    // Capture startup-only key snapshot + start time so health checks can
    // detect drift and report uptime
    if let Ok(mut g) = STARTUP_SNAPSHOT.lock() {
        let mut snap = std::collections::HashMap::new();
        for key in STARTUP_ONLY_KEYS {
            let v = user
                .get(*key)
                .or_else(|| system.get(*key))
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            snap.insert((*key).to_string(), v);
        }
        *g = Some(snap);
    }
    if let Ok(mut g) = ENGINE_STARTED_AT.lock() {
        *g = Some(Instant::now());
    }

    // Monitor for RPC-initiated shutdown requests
    tokio::spawn(async move {
        if rpc_shutdown_rx.recv().await.is_some() {
            log::info!("Shutdown requested via RPC");
            if let Err(e) = stop_engine().await {
                log::error!("Failed to stop engine via RPC shutdown: {}", e);
            }
        }
    });

    log::info!("Risuko engine started on port {}", rpc_port);
    Ok(())
}

pub async fn stop_engine() -> Result<(), Box<dyn std::error::Error>> {
    let mut guard = ENGINE_INSTANCE.lock().await;
    if let Some(mut instance) = guard.take() {
        // Stop periodic tasks
        if let Some(task) = instance.progress_task.take() {
            task.abort();
        }
        if let Some(task) = instance.auto_save_task.take() {
            task.abort();
        }
        if let Some(task) = instance.event_bridge_task.take() {
            task.abort();
        }

        // Stop RPC server
        instance.rpc_server.stop();

        // Shutdown manager (saves session, stops downloads, closes torrent engine)
        instance.manager.shutdown().await;
    }
    drop(guard);

    if let Ok(mut g) = ENGINE_STARTED_AT.lock() {
        *g = None;
    }

    log::info!("Risuko engine stopped");
    Ok(())
}

pub async fn restart_engine(
    config: &ConfigManager,
    event_sink: Arc<dyn EventSink>,
    storage: Arc<dyn StorageBackend>,
    upload_sinks: Option<Arc<UploadSinkManager>>,
) -> Result<(), Box<dyn std::error::Error>> {
    stop_engine().await?;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    start_engine(config, event_sink, storage, upload_sinks).await?;
    Ok(())
}

/// Get a handle to the task manager for direct calls
pub async fn get_manager() -> Option<Arc<TaskManager>> {
    let guard = ENGINE_INSTANCE.lock().await;
    guard.as_ref().map(|i| i.manager.clone())
}
