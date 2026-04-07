use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use tauri::AppHandle;

use crate::config;
use crate::engine::rss::RssManager;

pub struct AppState {
    pub config: Mutex<config::ConfigManager>,
    pub engine_running: Mutex<bool>,
    pub is_quitting: AtomicBool,
    pub rss: Mutex<Option<Arc<RssManager>>>,
}

impl AppState {
    pub fn new(handle: &AppHandle) -> Result<Self, Box<dyn std::error::Error>> {
        let config = config::ConfigManager::new(handle)?;
        let rss_manager = RssManager::new(handle);
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
