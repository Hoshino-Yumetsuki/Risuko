use std::path::PathBuf;
use std::sync::Arc;

use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde_json::{Map, Value};
use tokio::sync::Mutex;

use risuko_engine::config::defaults;
use risuko_engine::engine::events::EventBroadcaster;
use risuko_engine::engine::manager::TaskManager;
use risuko_engine::engine::options::EngineOptions;
use risuko_engine::engine::rpc::RpcServer;

// Global engine singleton

struct NapiEngine {
    manager: Arc<TaskManager>,
    rpc_server: RpcServer,
    events: EventBroadcaster,
    progress_task: tokio::task::JoinHandle<()>,
    auto_save_task: tokio::task::JoinHandle<()>,
}

static ENGINE: std::sync::LazyLock<Mutex<Option<NapiEngine>>> =
    std::sync::LazyLock::new(|| Mutex::new(None));

// JS-visible options

#[napi(object)]
pub struct EngineConfig {
    /// Custom config directory (default: OS config dir / dev.risuko.app)
    pub config_dir: Option<String>,
    /// RPC listen port
    pub rpc_port: Option<u16>,
    /// Whether to start the RPC server (default: true)
    pub enable_rpc: Option<bool>,
}

// Engine lifecycle

#[napi]
pub async fn start_engine(config: Option<EngineConfig>) -> Result<()> {
    let mut guard = ENGINE.lock().await;
    if guard.is_some() {
        return Err(Error::from_reason("Engine already running"));
    }

    let config_dir = config
        .as_ref()
        .and_then(|c| c.config_dir.as_deref())
        .map(PathBuf::from)
        .unwrap_or_else(default_config_dir);

    std::fs::create_dir_all(&config_dir)
        .map_err(|e| Error::from_reason(format!("Failed to create config dir: {}", e)))?;

    let system_config = load_config(&config_dir.join("system.json"), defaults::system_defaults());
    let user_config = load_config(&config_dir.join("user.json"), defaults::user_defaults());

    let mut options = EngineOptions::from_config(&system_config, &user_config);

    if let Some(port) = config.as_ref().and_then(|c| c.rpc_port) {
        options.set("rpc-listen-port".into(), Value::from(port));
    }

    let dir = options.dir();
    if !dir.is_empty() {
        std::fs::create_dir_all(&dir).ok();
    }

    let events = EventBroadcaster::default();
    let rpc_host = options.rpc_host();
    let rpc_port = options.rpc_listen_port();
    let rpc_secret = options.rpc_secret();

    let manager = Arc::new(
        TaskManager::new(&config_dir, options, events.clone())
            .await
            .map_err(|e| Error::from_reason(format!("Failed to create task manager: {}", e)))?,
    );

    let (rpc_shutdown_tx, _) = tokio::sync::mpsc::channel::<()>(1);

    let mut rpc_server = RpcServer::new(
        rpc_host,
        rpc_port,
        rpc_secret,
        manager.clone(),
        events.clone(),
        rpc_shutdown_tx,
    );

    let enable_rpc = config.as_ref().and_then(|c| c.enable_rpc).unwrap_or(true);
    if enable_rpc {
        rpc_server
            .start()
            .await
            .map_err(|e| Error::from_reason(format!("Failed to start RPC: {}", e)))?;
    }

    let mgr = manager.clone();
    let progress_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
        loop {
            interval.tick().await;
            mgr.update_progress().await;
        }
    });

    let mgr = manager.clone();
    let auto_save_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            interval.tick().await;
            if let Err(e) = mgr.save_session().await {
                log::warn!("Auto-save session failed: {}", e);
            }
        }
    });

    *guard = Some(NapiEngine {
        manager,
        rpc_server,
        events,
        progress_task,
        auto_save_task,
    });

    Ok(())
}

#[napi]
pub async fn stop_engine() -> Result<()> {
    let mut guard = ENGINE.lock().await;
    let mut engine = guard
        .take()
        .ok_or_else(|| Error::from_reason("Engine not running"))?;
    engine.progress_task.abort();
    engine.auto_save_task.abort();
    engine.rpc_server.stop();
    engine.manager.shutdown().await;
    Ok(())
}

// Task operations

async fn with_manager<F, Fut, T>(f: F) -> Result<T>
where
    F: FnOnce(Arc<TaskManager>) -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let guard = ENGINE.lock().await;
    let engine = guard
        .as_ref()
        .ok_or_else(|| Error::from_reason("Engine not running"))?;
    f(engine.manager.clone()).await
}

#[napi]
pub async fn add_uri(uris: Vec<String>, options: Option<serde_json::Value>) -> Result<String> {
    with_manager(|mgr| async move {
        let opts = to_map(options);
        mgr.add_http_task(uris, opts)
            .await
            .map_err(|e| Error::from_reason(e))
    })
    .await
}

#[napi]
pub async fn add_torrent(data: Buffer, options: Option<serde_json::Value>) -> Result<String> {
    with_manager(|mgr| async move {
        let opts = to_map(options);
        mgr.add_torrent_task(data.to_vec(), opts)
            .await
            .map_err(|e| Error::from_reason(e))
    })
    .await
}

#[napi]
pub async fn add_magnet(uri: String, options: Option<serde_json::Value>) -> Result<String> {
    with_manager(|mgr| async move {
        let opts = to_map(options);
        mgr.add_magnet_task(&uri, opts)
            .await
            .map_err(|e| Error::from_reason(e))
    })
    .await
}

#[napi]
pub async fn add_ed2k(uri: String, options: Option<serde_json::Value>) -> Result<String> {
    with_manager(|mgr| async move {
        let opts = to_map(options);
        mgr.add_ed2k_task(&uri, opts)
            .await
            .map_err(|e| Error::from_reason(e))
    })
    .await
}

#[napi]
pub async fn add_m3u8(uri: String, options: Option<serde_json::Value>) -> Result<String> {
    with_manager(|mgr| async move {
        let opts = to_map(options);
        mgr.add_m3u8_task(&uri, opts)
            .await
            .map_err(|e| Error::from_reason(e))
    })
    .await
}

#[napi]
pub async fn add_ftp(uri: String, options: Option<serde_json::Value>) -> Result<String> {
    with_manager(|mgr| async move {
        let opts = to_map(options);
        mgr.add_ftp_task(&uri, opts)
            .await
            .map_err(|e| Error::from_reason(e))
    })
    .await
}

// Control

#[napi]
pub async fn pause(gid: String) -> Result<()> {
    with_manager(|mgr| async move { mgr.pause(&gid).await.map_err(|e| Error::from_reason(e)) })
        .await
}

#[napi]
pub async fn unpause(gid: String) -> Result<()> {
    with_manager(|mgr| async move { mgr.unpause(&gid).await.map_err(|e| Error::from_reason(e)) })
        .await
}

#[napi]
pub async fn remove(gid: String) -> Result<()> {
    with_manager(|mgr| async move { mgr.remove(&gid).await.map_err(|e| Error::from_reason(e)) })
        .await
}

#[napi]
pub async fn pause_all() -> Result<()> {
    with_manager(|mgr| async move {
        mgr.pause_all().await;
        Ok(())
    })
    .await
}

#[napi]
pub async fn unpause_all() -> Result<()> {
    with_manager(|mgr| async move {
        mgr.unpause_all().await;
        Ok(())
    })
    .await
}

// Query

#[napi]
pub async fn tell_status(gid: String, keys: Option<Vec<String>>) -> Result<serde_json::Value> {
    with_manager(|mgr| async move {
        let k = keys.unwrap_or_default();
        mgr.tell_status(&gid, &k)
            .await
            .map_err(|e| Error::from_reason(e))
    })
    .await
}

#[napi]
pub async fn tell_active(keys: Option<Vec<String>>) -> Result<serde_json::Value> {
    with_manager(|mgr| async move {
        let k = keys.unwrap_or_default();
        Ok(mgr.tell_active(&k).await)
    })
    .await
}

#[napi]
pub async fn tell_waiting(
    offset: i32,
    num: u32,
    keys: Option<Vec<String>>,
) -> Result<serde_json::Value> {
    with_manager(|mgr| async move {
        let k = keys.unwrap_or_default();
        Ok(mgr.tell_waiting(offset as i64, num as usize, &k).await)
    })
    .await
}

#[napi]
pub async fn tell_stopped(
    offset: i32,
    num: u32,
    keys: Option<Vec<String>>,
) -> Result<serde_json::Value> {
    with_manager(|mgr| async move {
        let k = keys.unwrap_or_default();
        Ok(mgr.tell_stopped(offset as i64, num as usize, &k).await)
    })
    .await
}

#[napi]
pub async fn get_global_stat() -> Result<serde_json::Value> {
    with_manager(|mgr| async move { Ok(mgr.get_global_stat().await) }).await
}

#[napi]
pub async fn get_files(gid: String) -> Result<serde_json::Value> {
    with_manager(|mgr| async move { mgr.get_files(&gid).await.map_err(|e| Error::from_reason(e)) })
        .await
}

#[napi]
pub async fn get_peers(gid: String) -> Result<serde_json::Value> {
    with_manager(|mgr| async move { Ok(mgr.get_peers(&gid).await) }).await
}

#[napi]
pub async fn get_uris(gid: String) -> Result<serde_json::Value> {
    with_manager(|mgr| async move { mgr.get_uris(&gid).await.map_err(|e| Error::from_reason(e)) })
        .await
}

// Options

#[napi]
pub async fn get_option(gid: String) -> Result<serde_json::Value> {
    with_manager(|mgr| async move {
        mgr.get_option(&gid)
            .await
            .map_err(|e| Error::from_reason(e))
    })
    .await
}

#[napi]
pub async fn get_global_option() -> Result<serde_json::Value> {
    with_manager(|mgr| async move { Ok(mgr.get_global_option().await) }).await
}

#[napi]
pub async fn change_option(gid: String, options: serde_json::Value) -> Result<()> {
    with_manager(|mgr| async move {
        let opts = value_to_map(options);
        mgr.change_option(&gid, opts)
            .await
            .map_err(|e| Error::from_reason(e))
    })
    .await
}

#[napi]
pub async fn change_global_option(options: serde_json::Value) -> Result<()> {
    with_manager(|mgr| async move {
        let opts = value_to_map(options);
        mgr.change_global_option(opts).await;
        Ok(())
    })
    .await
}

// Session

#[napi]
pub async fn save_session() -> Result<()> {
    with_manager(|mgr| async move { mgr.save_session().await.map_err(|e| Error::from_reason(e)) })
        .await
}

#[napi]
pub async fn purge_download_result() -> Result<()> {
    with_manager(|mgr| async move {
        mgr.purge_download_result().await;
        Ok(())
    })
    .await
}

#[napi]
pub async fn remove_download_result(gid: String) -> Result<()> {
    with_manager(|mgr| async move {
        mgr.remove_download_result(&gid)
            .await
            .map_err(|e| Error::from_reason(e))
    })
    .await
}

// Events

/// Subscribe to engine events. The callback receives (eventName, gid).
/// Returns an unsubscribe function ID.
#[napi(ts_args_type = "callback: (eventName: string, gid: string) => void")]
pub fn on_event(
    callback: napi::threadsafe_function::ThreadsafeFunction<
        (String, String),
        napi::threadsafe_function::ErrorStrategy::Fatal,
    >,
) -> Result<()> {
    let _guard_handle = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to build event runtime");

        rt.block_on(async {
            let guard = ENGINE.lock().await;
            if let Some(engine) = guard.as_ref() {
                let mut rx = engine.events.subscribe();
                drop(guard);
                loop {
                    match rx.recv().await {
                        Ok(event) => {
                            let name = event.method_name().to_string();
                            let gid = event.gid().to_string();
                            callback.call(
                                (name, gid),
                                napi::threadsafe_function::ThreadsafeFunctionCallMode::NonBlocking,
                            );
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        Err(_) => continue, // lagged, skip
                    }
                }
            }
        });
    });

    Ok(())
}

// Helpers

fn default_config_dir() -> PathBuf {
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

fn to_map(val: Option<serde_json::Value>) -> Map<String, Value> {
    match val {
        Some(Value::Object(m)) => m,
        _ => Map::new(),
    }
}

fn value_to_map(val: serde_json::Value) -> Map<String, Value> {
    match val {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}
