use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Map, Value};

use risuko_engine::config::defaults;
use risuko_engine::engine::events::EventBroadcaster;
use risuko_engine::engine::manager::TaskManager;
use risuko_engine::engine::options::EngineOptions;
use risuko_engine::engine::rpc::RpcServer;

/// Start the engine in headless mode (no Tauri, no GUI).
/// Returns a handle to shut down when done.
pub async fn start_headless_engine(
    rpc_port: u16,
) -> Result<HeadlessEngine, Box<dyn std::error::Error>> {
    let config_dir = get_config_dir();
    std::fs::create_dir_all(&config_dir)?;

    let system_config = load_config(&config_dir.join("system.json"), defaults::system_defaults());
    let user_config = load_config(&config_dir.join("user.json"), defaults::user_defaults());

    let mut options = EngineOptions::from_config(&system_config, &user_config);
    // Override RPC port if specified
    options.set("rpc-listen-port".into(), Value::from(rpc_port));

    let dir = options.dir();
    if !dir.is_empty() {
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Failed to create download directory '{}': {}", dir, e))?;
    }

    let events = EventBroadcaster::default();
    let rpc_host = options.rpc_host();
    let rpc_secret = options.rpc_secret();
    let rpc_secret_clone = rpc_secret.clone();

    log::info!("Starting headless engine on port {}", rpc_port);

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
    let mgr_progress = manager.clone();
    let progress_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
        loop {
            interval.tick().await;
            mgr_progress.update_progress().await;
        }
    });

    // Start periodic session auto-save
    let mgr_save = manager.clone();
    let auto_save_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            interval.tick().await;
            if let Err(e) = mgr_save.save_session().await {
                log::warn!("Auto-save session failed: {}", e);
            }
        }
    });

    // Monitor for RPC-initiated shutdown requests
    let shutdown_notify = Arc::new(tokio::sync::Notify::new());
    let shutdown_notify_clone = shutdown_notify.clone();
    tokio::spawn(async move {
        if rpc_shutdown_rx.recv().await.is_some() {
            log::info!("Shutdown requested via RPC (headless)");
            shutdown_notify_clone.notify_one();
        }
    });

    Ok(HeadlessEngine {
        manager,
        rpc_server,
        progress_task,
        auto_save_task,
        shutdown_notify,
        rpc_secret: if rpc_secret_clone.is_empty() {
            None
        } else {
            Some(rpc_secret_clone)
        },
    })
}

pub struct HeadlessEngine {
    manager: Arc<TaskManager>,
    rpc_server: RpcServer,
    progress_task: tokio::task::JoinHandle<()>,
    auto_save_task: tokio::task::JoinHandle<()>,
    shutdown_notify: Arc<tokio::sync::Notify>,
    rpc_secret: Option<String>,
}

impl HeadlessEngine {
    /// Returns the RPC secret configured for this engine, if any.
    pub fn rpc_secret(&self) -> Option<&str> {
        self.rpc_secret.as_deref()
    }

    /// Returns a future that resolves when an RPC shutdown request is received.
    pub fn shutdown_requested(&self) -> impl std::future::Future<Output = ()> + '_ {
        self.shutdown_notify.notified()
    }

    pub async fn shutdown(mut self) {
        self.progress_task.abort();
        self.auto_save_task.abort();
        self.rpc_server.stop();
        self.manager.shutdown().await;
        log::info!("Headless engine stopped");
    }
}

fn get_config_dir() -> PathBuf {
    dirs::config_dir()
        .map(|d| d.join("dev.risuko.app"))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn load_config(path: &std::path::Path, defaults: Map<String, Value>) -> Map<String, Value> {
    if let Ok(data) = std::fs::read_to_string(path) {
        if let Ok(Value::Object(mut map)) = serde_json::from_str(&data) {
            for (k, v) in &defaults {
                if !map.contains_key(k) {
                    map.insert(k.clone(), v.clone());
                }
            }
            return map;
        }
    }
    defaults
}
