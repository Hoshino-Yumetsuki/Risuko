use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use risuko_engine::config::ConfigManager;
use risuko_engine::engine::rss::RssManager;
use risuko_engine::traits::{ConfigDirProvider, StorageBackend};

pub struct AppState {
    pub config: Mutex<ConfigManager>,
    pub engine_running: Mutex<bool>,
    pub is_quitting: AtomicBool,
    pub rss: Mutex<Option<Arc<RssManager>>>,
}

impl AppState {
    pub fn new(
        config_dir_provider: &dyn ConfigDirProvider,
        storage: Arc<dyn StorageBackend>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let config = ConfigManager::new(config_dir_provider)?;
        let event_sink: Arc<dyn risuko_engine::EventSink> = Arc::new(risuko_engine::NoopEventSink);
        let rss_manager = RssManager::new(storage, event_sink);
        if let Err(e) = rss_manager.load() {
            log::warn!("Failed to load RSS data: {}", e);
        }
        Ok(Self {
            config: Mutex::new(config),
            engine_running: Mutex::new(false),
            is_quitting: AtomicBool::new(false),
            rss: Mutex::new(Some(Arc::new(rss_manager))),
        })
    }
}
