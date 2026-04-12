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
            v.as_u64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
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
            .and_then(|v| {
                v.as_f64()
                    .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            })
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_system() -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("dir".into(), json!("/downloads"));
        m.insert("max-concurrent-downloads".into(), json!(3));
        m.insert("rpc-listen-port".into(), json!(16800));
        m.insert("rpc-secret".into(), json!("secret123"));
        m.insert("seed-ratio".into(), json!("1.5"));
        m.insert("ed2k-server".into(), json!("srv1,srv2"));
        m.insert("ed2k-port".into(), json!(5662));
        m
    }

    fn make_user() -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("rpc-host".into(), json!("0.0.0.0"));
        m.insert("m3u8-output-format".into(), json!("mp4"));
        // This key should be ignored (not in the allow list)
        m.insert("dir".into(), json!("/user-override"));
        m
    }

    // --- from_config ---

    #[test]
    fn from_config_copies_system_keys() {
        let opts = EngineOptions::from_config(&make_system(), &Map::new());
        assert_eq!(opts.dir(), "/downloads");
        assert_eq!(opts.max_concurrent_downloads(), 3);
    }

    #[test]
    fn from_config_applies_user_overrides() {
        let opts = EngineOptions::from_config(&make_system(), &make_user());
        assert_eq!(opts.rpc_host(), "0.0.0.0");
        assert_eq!(opts.get_str("m3u8-output-format"), Some("mp4"));
        // dir should NOT be overridden from user config
        assert_eq!(opts.dir(), "/downloads");
    }

    // --- getters with defaults ---

    #[test]
    fn getter_defaults_when_empty() {
        let opts = EngineOptions::from_config(&Map::new(), &Map::new());
        assert_eq!(opts.dir(), ".");
        assert_eq!(opts.max_concurrent_downloads(), 5);
        assert_eq!(opts.rpc_listen_port(), 16800);
        assert_eq!(opts.rpc_host(), "127.0.0.1");
        assert_eq!(opts.rpc_secret(), "");
        assert_eq!(opts.seed_ratio(), 0.0);
        assert_eq!(opts.seed_time(), 0);
        assert!(opts.ed2k_servers().is_empty());
        assert_eq!(opts.ed2k_port(), 4662);
    }

    #[test]
    fn getter_values_from_config() {
        let opts = EngineOptions::from_config(&make_system(), &Map::new());
        assert_eq!(opts.rpc_secret(), "secret123");
        assert_eq!(opts.seed_ratio(), 1.5);
        assert_eq!(opts.ed2k_servers(), vec!["srv1", "srv2"]);
        assert_eq!(opts.ed2k_port(), 5662);
    }

    #[test]
    fn get_u64_parses_string() {
        let mut sys = Map::new();
        sys.insert("rpc-listen-port".into(), json!("9999"));
        let opts = EngineOptions::from_config(&sys, &Map::new());
        assert_eq!(opts.rpc_listen_port(), 9999);
    }

    // --- set ---

    #[test]
    fn set_overrides_value() {
        let mut opts = EngineOptions::from_config(&make_system(), &Map::new());
        opts.set("dir".into(), json!("/new"));
        assert_eq!(opts.dir(), "/new");
    }

    // --- merge_task_options ---

    #[test]
    fn merge_task_options_overrides_globals() {
        let opts = EngineOptions::from_config(&make_system(), &Map::new());
        let mut task = Map::new();
        task.insert("dir".into(), json!("/task-dir"));
        task.insert("out".into(), json!("file.zip"));

        let merged = opts.merge_task_options(&task);
        assert_eq!(merged.get("dir").unwrap(), "/task-dir");
        assert_eq!(merged.get("out").unwrap(), "file.zip");
        // Original global key preserved
        assert_eq!(merged.get("rpc-secret").unwrap(), "secret123");
    }

    #[test]
    fn merge_task_options_empty_task_returns_globals() {
        let opts = EngineOptions::from_config(&make_system(), &Map::new());
        let merged = opts.merge_task_options(&Map::new());
        assert_eq!(merged.get("dir").unwrap(), "/downloads");
    }
}
