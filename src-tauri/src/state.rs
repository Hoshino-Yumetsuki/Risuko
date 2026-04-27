use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use risuko_engine::config::ConfigManager;
use risuko_engine::engine::rss::RssManager;
use risuko_engine::engine::upload::UploadSinkManager;
use risuko_engine::traits::StorageBackend;

pub struct AppState {
    pub config: Mutex<ConfigManager>,
    pub engine_running: Mutex<bool>,
    pub is_quitting: AtomicBool,
    pub rss: Mutex<Option<Arc<RssManager>>>,
    pub upload_sinks: Mutex<Option<Arc<UploadSinkManager>>>,
    pub log_dir: PathBuf,
    /// Keeps the file logger alive; dropped when AppState is dropped.
    pub _log_guard: Option<tracing_appender::non_blocking::WorkerGuard>,
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
            log::warn!("Failed to load RSS data: {}", e);
        }
        let upload_manager = UploadSinkManager::new(storage, event_sink);
        if let Err(e) = upload_manager.load() {
            log::warn!("Failed to load upload sinks: {}", e);
        }
        Ok(Self {
            config: Mutex::new(config),
            engine_running: Mutex::new(false),
            is_quitting: AtomicBool::new(false),
            rss: Mutex::new(Some(Arc::new(rss_manager))),
            upload_sinks: Mutex::new(Some(Arc::new(upload_manager))),
            log_dir,
            _log_guard: Some(log_guard),
        })
    }
}
