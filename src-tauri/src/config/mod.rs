pub mod defaults;

use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

pub struct ConfigManager {
    system_config: Map<String, Value>,
    user_config: Map<String, Value>,
    config_dir: PathBuf,
}

impl ConfigManager {
    pub fn new(handle: &AppHandle) -> Result<Self, Box<dyn std::error::Error>> {
        let config_dir = get_config_dir(handle);
        fs::create_dir_all(&config_dir)?;

        let system_config = load_or_default(
            &config_dir.join("system.json"),
            defaults::system_defaults(),
        );
        let user_config = load_or_default(
            &config_dir.join("user.json"),
            defaults::user_defaults(),
        );

        Ok(Self {
            system_config,
            user_config,
            config_dir,
        })
    }

    pub fn get_system_config(&self) -> &Map<String, Value> {
        &self.system_config
    }

    pub fn get_user_config(&self) -> &Map<String, Value> {
        &self.user_config
    }

    pub fn get_merged_config(&self) -> Value {
        let mut merged = Map::new();
        for (k, v) in &self.system_config {
            merged.insert(k.clone(), v.clone());
        }
        for (k, v) in &self.user_config {
            merged.insert(k.clone(), v.clone());
        }

        // Add runtime context.
        merged.insert("platform".into(), json!(std::env::consts::OS));
        merged.insert("arch".into(), json!(std::env::consts::ARCH));

        Value::Object(merged)
    }

    pub fn set_system_config_map(&mut self, map: &Map<String, Value>) {
        for (k, v) in map {
            self.system_config.insert(k.clone(), v.clone());
        }
        self.save_system();
    }

    pub fn set_user_config_map(&mut self, map: &Map<String, Value>) {
        for (k, v) in map {
            self.user_config.insert(k.clone(), v.clone());
        }
        self.save_user();
    }

    pub fn reset(&mut self) {
        self.system_config = defaults::system_defaults();
        self.user_config = defaults::user_defaults();
        self.save_system();
        self.save_user();
    }

    fn save_system(&self) {
        let path = self.config_dir.join("system.json");
        let data = serde_json::to_string_pretty(&self.system_config).unwrap_or_default();
        let _ = fs::write(path, data);
    }

    fn save_user(&self) {
        let path = self.config_dir.join("user.json");
        let data = serde_json::to_string_pretty(&self.user_config).unwrap_or_default();
        let _ = fs::write(path, data);
    }
}

fn get_config_dir(handle: &AppHandle) -> PathBuf {
    handle
        .path()
        .app_config_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
}

fn load_or_default(path: &Path, defaults: Map<String, Value>) -> Map<String, Value> {
    if let Ok(data) = fs::read_to_string(path) {
        if let Ok(Value::Object(mut map)) = serde_json::from_str(&data) {
            // Fill in missing keys from defaults.
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
