use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::speed_limiter::parse_speed_limit;

fn is_reserved_engine_key(key: &str) -> bool {
    let normalized = key.to_ascii_lowercase();
    normalized.starts_with("rpc-")
        || normalized.ends_with("-port")
        || normalized.ends_with("-secret")
}

fn apply_engine_overrides(global: &mut Map<String, Value>, user: &Map<String, Value>) {
    if let Some(Value::Object(overrides)) = user.get("engine-overrides") {
        for (k, v) in overrides {
            if is_reserved_engine_key(k) {
                continue;
            }
            global.insert(k.clone(), v.clone());
        }
    } else if let Some(value) = user.get("engine-overrides") {
        tracing::warn!(
            "Ignoring invalid engine-overrides value: expected object, got {}",
            value
        );
    }
}

/// Default global options and per-task option management
/// Maps aria2 option names to internal config values

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineOptions {
    /// Global options (applied to all new tasks as defaults)
    pub global: Map<String, Value>,
}

impl EngineOptions {
    pub fn from_config(system: &Map<String, Value>, user: &Map<String, Value>) -> Self {
        // Relevant system config becomes the global engine options
        let mut global = system.clone();

        // Apply user overrides that affect engine behavior
        for key in [
            "rpc-host",
            "m3u8-output-format",
            "keep-seeding",
            "bt-create-subfolder",
            "bt-enable-upnp",
            "bt-upnp-lease",
            "bt-encryption-policy",
            "bt-listen-v6",
            "bt-enable-lsd",
            "purge-record-on-start",
            "task-routing-rules",
            "file-category-dirs",
            "max-worker-retries",
        ] {
            if let Some(v) = user.get(key) {
                global.insert(key.into(), v.clone());
            }
        }

        // Advanced escape hatch: allow users to provide arbitrary engine keys
        // from the UI via `engine-overrides` so newly added backend options can
        // be configured without waiting for dedicated form fields
        apply_engine_overrides(&mut global, user);

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

    /// Coerce common boolean representations. Accepts native bools,
    /// "true"/"false" strings, "1"/"0" strings, and numeric 0/1
    pub fn get_bool(&self, key: &str) -> Option<bool> {
        match self.global.get(key)? {
            Value::Bool(b) => Some(*b),
            Value::String(s) => match s.as_str() {
                "true" | "1" | "yes" => Some(true),
                "false" | "0" | "no" => Some(false),
                _ => None,
            },
            Value::Number(n) => n.as_u64().map(|v| v != 0),
            _ => None,
        }
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

    /// Keep seeding until the user stops manually. Overrides seed-time /
    /// seed-ratio enforcement (those only take effect if this is false)
    pub fn keep_seeding(&self) -> bool {
        self.get_bool("keep-seeding").unwrap_or(false)
    }

    /// Max outstanding chunk requests per peer. 0 or missing = use crate default
    pub fn bt_max_outstanding_per_peer(&self) -> Option<usize> {
        self.get_u64("bt-max-outstanding-per-peer")
            .filter(|&v| v != 0)
            .map(|v| v as usize)
    }

    /// Max concurrent peer connections per torrent. 0 or missing = use crate default
    pub fn bt_max_peers_per_torrent(&self) -> Option<usize> {
        self.get_u64("bt-max-peers-per-torrent")
            .filter(|&v| v != 0)
            .map(|v| v as usize)
    }
    pub fn bt_upload_rate_limit(&self) -> Option<u64> {
        self.get_u64("bt-upload-rate-limit").filter(|&v| v != 0)
    }

    /// UPnP IGD port forwarding for the BitTorrent listener. Defaults to on
    pub fn bt_enable_upnp(&self) -> bool {
        self.get_bool("bt-enable-upnp").unwrap_or(true)
    }

    /// UPnP mapping lease duration in seconds. 0 or missing = use crate default (300)
    pub fn bt_upnp_lease(&self) -> Option<std::time::Duration> {
        self.get_u64("bt-upnp-lease")
            .filter(|&v| v != 0)
            .map(std::time::Duration::from_secs)
    }

    /// BEP-8 Message Stream Encryption policy: "plaintext", "prefer", "require"
    /// Defaults to `prefer` (MSE first, plaintext fallback)
    pub fn bt_encryption_policy(&self) -> &'static str {
        match self.get_str("bt-encryption-policy").unwrap_or("prefer") {
            "plaintext" => "plaintext",
            "require" => "require",
            _ => "prefer",
        }
    }

    /// Also bind an IPv6 TCP listener. Defaults to off
    pub fn bt_listen_v6(&self) -> bool {
        self.get_bool("bt-listen-v6").unwrap_or(false)
    }

    /// BEP-14 Local Service Discovery. Defaults to on
    pub fn bt_enable_lsd(&self) -> bool {
        self.get_bool("bt-enable-lsd").unwrap_or(true)
    }

    /// Purge completed/stopped download records when the engine starts
    pub fn purge_record_on_start(&self) -> bool {
        self.get_bool("purge-record-on-start").unwrap_or(false)
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

    /// User-defined task routing rules (pattern -> tag + directory)
    pub fn task_routing_rules(&self) -> Vec<super::routing::TaskRoutingRule> {
        self.global
            .get("task-routing-rules")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default()
    }

    /// Legacy file-category → directory map (e.g. { music: "/Music" })
    pub fn file_category_dirs(&self) -> std::collections::HashMap<String, String> {
        self.global
            .get("file-category-dirs")
            .and_then(|v| {
                if let Value::Object(map) = v {
                    let mut result = std::collections::HashMap::new();
                    for (k, val) in map {
                        if let Some(s) = val.as_str() {
                            result.insert(k.clone(), s.to_string());
                        }
                    }
                    Some(result)
                } else {
                    None
                }
            })
            .unwrap_or_default()
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

    // -- from_config --

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

    #[test]
    fn from_config_applies_engine_overrides_map() {
        let mut user = Map::new();
        user.insert(
            "engine-overrides".into(),
            json!({
                "dir": "/override-dir",
                "max-concurrent-downloads": 12,
                "rpc-host": "10.0.0.2",
                "rpc-listen-port": 17000,
                "rpc-secret": "override-secret"
            }),
        );

        let opts = EngineOptions::from_config(&make_system(), &user);
        assert_eq!(opts.dir(), "/override-dir");
        assert_eq!(opts.max_concurrent_downloads(), 12);
        assert_eq!(opts.rpc_host(), "127.0.0.1");
        assert_eq!(opts.rpc_listen_port(), 16800);
        assert_eq!(opts.rpc_secret(), "secret123");
    }

    // -- getters with defaults --

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

    // -- set --

    #[test]
    fn set_overrides_value() {
        let mut opts = EngineOptions::from_config(&make_system(), &Map::new());
        opts.set("dir".into(), json!("/new"));
        assert_eq!(opts.dir(), "/new");
    }

    // -- merge_task_options --

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

    // -- BT accessors --

    #[test]
    fn get_bool_accepts_native_strings_and_numbers() {
        let mut sys = Map::new();
        sys.insert("a".into(), json!(true));
        sys.insert("b".into(), json!("true"));
        sys.insert("c".into(), json!("yes"));
        sys.insert("d".into(), json!("1"));
        sys.insert("e".into(), json!(1));
        sys.insert("f".into(), json!(false));
        sys.insert("g".into(), json!("false"));
        sys.insert("h".into(), json!("no"));
        sys.insert("i".into(), json!("0"));
        sys.insert("j".into(), json!(0));
        sys.insert("k".into(), json!("garbage"));
        let opts = EngineOptions::from_config(&sys, &Map::new());
        for k in ["a", "b", "c", "d", "e"] {
            assert_eq!(opts.get_bool(k), Some(true), "{k}");
        }
        for k in ["f", "g", "h", "i", "j"] {
            assert_eq!(opts.get_bool(k), Some(false), "{k}");
        }
        assert_eq!(opts.get_bool("k"), None);
        assert_eq!(opts.get_bool("missing"), None);
    }

    #[test]
    fn bt_enable_upnp_defaults_true() {
        let opts = EngineOptions::from_config(&Map::new(), &Map::new());
        assert!(opts.bt_enable_upnp());
        let mut sys = Map::new();
        sys.insert("bt-enable-upnp".into(), json!(false));
        let opts = EngineOptions::from_config(&sys, &Map::new());
        assert!(!opts.bt_enable_upnp());
    }

    #[test]
    fn bt_upnp_lease_zero_means_default() {
        let opts = EngineOptions::from_config(&Map::new(), &Map::new());
        assert_eq!(opts.bt_upnp_lease(), None);
        let mut sys = Map::new();
        sys.insert("bt-upnp-lease".into(), json!(0));
        let opts = EngineOptions::from_config(&sys, &Map::new());
        assert_eq!(opts.bt_upnp_lease(), None);
        let mut sys = Map::new();
        sys.insert("bt-upnp-lease".into(), json!(120));
        let opts = EngineOptions::from_config(&sys, &Map::new());
        assert_eq!(
            opts.bt_upnp_lease(),
            Some(std::time::Duration::from_secs(120))
        );
    }

    #[test]
    fn bt_encryption_policy_normalises_unknown_to_prefer() {
        let opts = EngineOptions::from_config(&Map::new(), &Map::new());
        assert_eq!(opts.bt_encryption_policy(), "prefer");
        for (set, want) in [
            ("plaintext", "plaintext"),
            ("prefer", "prefer"),
            ("require", "require"),
            ("nonsense", "prefer"),
            ("REQUIRE", "prefer"), // case sensitive on purpose
        ] {
            let mut sys = Map::new();
            sys.insert("bt-encryption-policy".into(), json!(set));
            let opts = EngineOptions::from_config(&sys, &Map::new());
            assert_eq!(opts.bt_encryption_policy(), want, "set={set}");
        }
    }

    #[test]
    fn bt_listen_v6_defaults_false() {
        let opts = EngineOptions::from_config(&Map::new(), &Map::new());
        assert!(!opts.bt_listen_v6());
        let mut sys = Map::new();
        sys.insert("bt-listen-v6".into(), json!(true));
        let opts = EngineOptions::from_config(&sys, &Map::new());
        assert!(opts.bt_listen_v6());
    }

    #[test]
    fn bt_enable_lsd_defaults_true() {
        let opts = EngineOptions::from_config(&Map::new(), &Map::new());
        assert!(opts.bt_enable_lsd());
        let mut sys = Map::new();
        sys.insert("bt-enable-lsd".into(), json!(false));
        let opts = EngineOptions::from_config(&sys, &Map::new());
        assert!(!opts.bt_enable_lsd());
    }

    #[test]
    fn purge_record_on_start_defaults_false() {
        let opts = EngineOptions::from_config(&Map::new(), &Map::new());
        assert!(!opts.purge_record_on_start());
    }

    #[test]
    fn purge_record_on_start_from_bool_value() {
        let mut sys = Map::new();
        sys.insert("purge-record-on-start".into(), json!(true));
        let opts = EngineOptions::from_config(&sys, &Map::new());
        assert!(opts.purge_record_on_start());

        let mut sys = Map::new();
        sys.insert("purge-record-on-start".into(), json!(false));
        let opts = EngineOptions::from_config(&sys, &Map::new());
        assert!(!opts.purge_record_on_start());
    }

    #[test]
    fn purge_record_on_start_from_string_coercion() {
        for (set, want) in [
            ("true", true),
            ("false", false),
            ("yes", true),
            ("no", false),
            ("1", true),
            ("0", false),
        ] {
            let mut sys = Map::new();
            sys.insert("purge-record-on-start".into(), json!(set));
            let opts = EngineOptions::from_config(&sys, &Map::new());
            assert_eq!(
                opts.purge_record_on_start(),
                want,
                "string value '{set}' should coerce to {want}"
            );
        }
    }
}
