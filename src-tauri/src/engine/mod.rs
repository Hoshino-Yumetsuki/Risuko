pub mod ed2k;
pub mod events;
pub mod ftp;
pub mod http;
pub mod m3u8;
pub mod manager;
pub mod options;
pub mod rpc;
pub mod rss;
pub mod session;
pub mod speed_limiter;
pub mod task;
pub mod torrent;

pub use session::SESSION_FILENAME;

/// Suffix for per-chunk resume metadata sidecar file
pub const CHUNK_META_SUFFIX: &str = ".chunks";

use std::path::PathBuf;
use std::sync::Arc;
use tauri::AppHandle;
use tauri::Emitter;
use tauri::Manager;
use tokio::sync::Mutex;

use crate::state::AppState;

use self::events::EventBroadcaster;
use self::manager::TaskManager;
use self::options::EngineOptions;
use self::rpc::RpcServer;

static ENGINE_INSTANCE: std::sync::LazyLock<Mutex<Option<EngineInstance>>> =
    std::sync::LazyLock::new(|| Mutex::new(None));

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

fn should_start_embedded_engine(config: &crate::config::ConfigManager) -> bool {
    let host = config
        .get_user_config()
        .get("rpc-host")
        .and_then(|v| v.as_str())
        .unwrap_or("127.0.0.1")
        .trim()
        .to_lowercase();

    is_local_rpc_host(host.as_str())
}

pub async fn start_engine(handle: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let state = handle.state::<AppState>();

    {
        let config = state.config.lock().unwrap();
        if !should_start_embedded_engine(&config) {
            let host = config
                .get_user_config()
                .get("rpc-host")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            log::info!(
                "Skipping embedded engine startup because rpc-host is set to external address: {}",
                host
            );
            *state.engine_running.lock().unwrap() = false;
            return Ok(());
        }
    }

    // Check if already running
    {
        let guard = ENGINE_INSTANCE.lock().await;
        if guard.is_some() {
            log::info!("Engine already running");
            *state.engine_running.lock().unwrap() = true;
            return Ok(());
        }
    }

    let (options, config_dir) = {
        let config = state.config.lock().unwrap();
        let system = config.get_system_config();
        let user = config.get_user_config();
        let options = EngineOptions::from_config(system, user);
        let config_dir = handle
            .path()
            .app_config_dir()
            .unwrap_or_else(|_| PathBuf::from("."));
        (options, config_dir)
    };

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

    log::info!("Starting Motrix engine (in-process)");

    let manager = Arc::new(
        TaskManager::new(&config_dir, options, events.clone())
            .await
            .map_err(|e| format!("Failed to create task manager: {}", e))?,
    );

    let (rpc_shutdown_tx, mut rpc_shutdown_rx) = tokio::sync::mpsc::channel::<()>(1);

    let mut rpc_server = RpcServer::new(rpc_host, rpc_port, rpc_secret, manager.clone(), events.clone(), rpc_shutdown_tx);
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

    // Bridge engine events to Tauri events so the frontend can use listen()
    let event_handle = handle.clone();
    let mut event_rx = events.subscribe();
    let event_bridge_task = tokio::spawn(async move {
        use events::EngineEvent;
        loop {
            match event_rx.recv().await {
                Ok(event) => {
                    let (name, gid) = match &event {
                        EngineEvent::DownloadStart { gid } => ("engine:download-start", gid.as_str()),
                        EngineEvent::DownloadPause { gid } => ("engine:download-pause", gid.as_str()),
                        EngineEvent::DownloadStop { gid } => ("engine:download-stop", gid.as_str()),
                        EngineEvent::DownloadComplete { gid } => ("engine:download-complete", gid.as_str()),
                        EngineEvent::DownloadError { gid } => ("engine:download-error", gid.as_str()),
                        EngineEvent::BtDownloadComplete { gid } => ("engine:bt-download-complete", gid.as_str()),
                    };
                    let payload = serde_json::json!({ "gid": gid });
                    if let Err(e) = event_handle.emit(name, payload) {
                        log::warn!("Failed to emit Tauri event {}: {}", name, e);
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
    *state.engine_running.lock().unwrap() = true;

    // Monitor for RPC-initiated shutdown requests (spawned after instance is stored)
    let shutdown_handle = handle.clone();
    tokio::spawn(async move {
        if rpc_shutdown_rx.recv().await.is_some() {
            log::info!("Shutdown requested via RPC");
            if let Err(e) = stop_engine(&shutdown_handle).await {
                log::error!("Failed to stop engine via RPC shutdown: {}", e);
            }
        }
    });

    log::info!("Motrix engine started on port {}", rpc_port);
    Ok(())
}

pub async fn stop_engine(handle: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
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

    let state = handle.state::<AppState>();
    *state.engine_running.lock().unwrap() = false;
    log::info!("Motrix engine stopped");
    Ok(())
}

pub async fn restart_engine(handle: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    stop_engine(handle).await?;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    start_engine(handle).await?;
    Ok(())
}

/// Get a handle to the task manager for direct calls from Tauri commands
pub async fn get_manager() -> Option<Arc<TaskManager>> {
    let guard = ENGINE_INSTANCE.lock().await;
    guard.as_ref().map(|i| i.manager.clone())
}
