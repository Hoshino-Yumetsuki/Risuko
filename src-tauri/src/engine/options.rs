use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::speed_limiter::parse_speed_limit;

/// Default global options and per-task option management
/// Maps aria2 option names to internal config values

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineOptions {
    /// Global options (applied to all new tasks as defaults)
    pub global: Map<String, Value>,
}

impl EngineOptions {
    pub fn from_config(system: &Map<String, Value>, user: &Map<String, Value>) -> Self {
        let mut global = Map::new();

        // Copy relevant system config as global engine options
        for (k, v) in system {
            global.insert(k.clone(), v.clone());
        }

        // Apply user overrides that affect engine behavior
        for key in ["rpc-host", "m3u8-output-format"] {
            if let Some(v) = user.get(key) {
                global.insert(key.into(), v.clone());
            }
        }

        Self { global }
    }

    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.global.get(key).and_then(|v| v.as_str())
    }

    pub fn get_u64(&self, key: &str) -> Option<u64> {
        self.global.get(key).and_then(|v| {
            v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        })
    }

    pub fn set(&mut self, key: String, value: Value) {
        self.global.insert(key, value);
    }

    pub fn dir(&self) -> String {
        self.get_str("dir").unwrap_or(".").to_string()
    }

    pub fn max_concurrent_downloads(&self) -> usize {
        self.get_u64("max-concurrent-downloads").unwrap_or(5) as usize
    }

    pub fn max_overall_download_limit(&self) -> u64 {
        self.global
            .get("max-overall-download-limit")
            .map(parse_speed_limit)
            .unwrap_or(0)
    }

    pub fn rpc_listen_port(&self) -> u16 {
        self.get_u64("rpc-listen-port").unwrap_or(16800) as u16
    }

    pub fn rpc_host(&self) -> String {
        self.get_str("rpc-host").unwrap_or("127.0.0.1").to_string()
    }

    pub fn rpc_secret(&self) -> String {
        self.get_str("rpc-secret").unwrap_or("").to_string()
    }

    pub fn seed_ratio(&self) -> f64 {
        self.global
            .get("seed-ratio")
            .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
            .unwrap_or(0.0)
    }

    pub fn seed_time(&self) -> u64 {
        self.get_u64("seed-time").unwrap_or(0)
    }

    pub fn ed2k_servers(&self) -> Vec<String> {
        self.get_str("ed2k-server")
            .unwrap_or("")
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }

    pub fn ed2k_port(&self) -> u16 {
        self.get_u64("ed2k-port").unwrap_or(4662) as u16
    }

    /// Merge per-task options over global defaults, returning a combined map
    pub fn merge_task_options(&self, task_opts: &Map<String, Value>) -> Map<String, Value> {
        let mut merged = self.global.clone();
        for (k, v) in task_opts {
            merged.insert(k.clone(), v.clone());
        }
        merged
    }
}
