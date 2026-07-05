use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use risuko_engine::config::ConfigManager;
use risuko_engine::engine::rss::RssManager;
use risuko_engine::engine::stats::DownloadStatsManager;
use risuko_engine::engine::upload::UploadSinkManager;
use risuko_engine::traits::StorageBackend;

use crate::managers::vault::VaultManager;

fn config_bool(value: Option<&serde_json::Value>) -> bool {
    value
        .and_then(|v| match v {
            serde_json::Value::Bool(flag) => Some(*flag),
            serde_json::Value::String(text) => {
                let normalized = text.trim().to_ascii_lowercase();
                Some(matches!(normalized.as_str(), "1" | "true" | "yes" | "on"))
            }
            serde_json::Value::Number(number) => number
                .as_i64()
                .map(|n| n != 0)
                .or_else(|| number.as_f64().map(|n| n != 0.0)),
            _ => None,
        })
        .unwrap_or(false)
}

pub struct AppState {
    pub config: Mutex<ConfigManager>,
    pub is_quitting: AtomicBool,
    pub rss: Arc<RssManager>,
    pub stats: Arc<DownloadStatsManager>,
    pub upload_sinks: Arc<UploadSinkManager>,
    pub vault: Arc<VaultManager>,
    pub log_dir: PathBuf,
    pub last_clipboard_self_write: Mutex<Option<String>>,
    pub last_clipboard_seen: Mutex<Option<String>>,
    pub pending_clip_uri: Mutex<Option<String>>,
    #[cfg(not(target_os = "android"))]
    pub tray_anchor: Mutex<Option<(f64, f64, f64, f64)>>,
    pub _log_guard: tracing_appender::non_blocking::WorkerGuard,
}

impl AppState {
    pub fn new(
        config: ConfigManager,
        storage: Arc<dyn StorageBackend>,
        log_dir: PathBuf,
        log_guard: tracing_appender::non_blocking::WorkerGuard,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let event_sink: Arc<dyn risuko_engine::EventSink> = Arc::new(risuko_engine::NoopEventSink);
        let rss_manager = RssManager::new(storage.clone(), event_sink.clone());
        if let Err(e) = rss_manager.load() {
            tracing::warn!("Failed to load RSS data: {}", e);
        }
        let stats_manager = DownloadStatsManager::new(storage.clone());
        if let Err(e) = stats_manager.load() {
            tracing::warn!("Failed to load download stats: {}", e);
        }
        if config_bool(config.get_user_config().get("purge-record-on-start")) {
            if let Err(e) = stats_manager.clear_sync() {
                tracing::warn!("Failed to clear download stats on startup: {}", e);
            }
        }
        let upload_manager = UploadSinkManager::new(storage, event_sink);
        if let Err(e) = upload_manager.load() {
            tracing::warn!("Failed to load upload sinks: {}", e);
        }
        Ok(Self {
            config: Mutex::new(config),
            is_quitting: AtomicBool::new(false),
            rss: Arc::new(rss_manager),
            stats: Arc::new(stats_manager),
            upload_sinks: Arc::new(upload_manager),
            vault: Arc::new(VaultManager::new()),
            log_dir,
            last_clipboard_self_write: Mutex::new(None),
            last_clipboard_seen: Mutex::new(None),
            pending_clip_uri: Mutex::new(None),
            #[cfg(not(target_os = "android"))]
            tray_anchor: Mutex::new(None),
            _log_guard: log_guard,
        })
    }
}
