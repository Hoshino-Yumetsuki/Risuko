use std::sync::Mutex;
use tauri::AppHandle;

use crate::config;

pub struct AppState {
    pub config: Mutex<config::ConfigManager>,
    pub engine_running: Mutex<bool>,
}

impl AppState {
    pub fn new(handle: &AppHandle) -> Result<Self, Box<dyn std::error::Error>> {
        let config = config::ConfigManager::new(handle)?;
        Ok(Self {
            config: Mutex::new(config),
            engine_running: Mutex::new(false),
        })
    }
}
