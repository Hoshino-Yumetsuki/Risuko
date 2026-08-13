pub mod defaults;

use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

use crate::traits::ConfigDirProvider;

pub struct ConfigManager {
    system_config: Map<String, Value>,
    user_config: Map<String, Value>,
    config_dir: PathBuf,
}

impl ConfigManager {
    pub fn new(provider: &dyn ConfigDirProvider) -> Result<Self, Box<dyn std::error::Error>> {
        Self::with_dir(provider.config_dir())
    }

    /// Create a ConfigManager with an explicit config directory path
    pub fn with_dir(config_dir: PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        fs::create_dir_all(&config_dir)?;

        let system_config =
            load_or_default(&config_dir.join("system.json"), defaults::system_defaults());
        let user_config = load_or_default(&config_dir.join("user.json"), defaults::user_defaults());

        let mut manager = Self {
            system_config,
            user_config,
            config_dir,
        };

        if manager.migrate_legacy_keep_seeding_defaults() {
            if let Err(err) = manager.save_system() {
                tracing::warn!(
                    "Failed to persist legacy keep-seeding migration; continuing startup: {}",
                    err
                );
            }
        }

        Ok(manager)
    }

    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    pub fn get_system_config(&self) -> &Map<String, Value> {
        &self.system_config
    }

    pub fn get_user_config(&self) -> &Map<String, Value> {
        &self.user_config
    }

    pub fn get_merged_config(&self) -> Value {
        let mut merged = self.system_config.clone();
        merged.extend(self.user_config.clone());

        // Add runtime context without clobbering a user/system key of the same name (insert only when absent)
        merged
            .entry("platform".to_string())
            .or_insert_with(|| json!(std::env::consts::OS));
        merged
            .entry("arch".to_string())
            .or_insert_with(|| json!(std::env::consts::ARCH));

        Value::Object(merged)
    }

    pub fn set_system_config_map(&mut self, map: &Map<String, Value>) -> Result<(), String> {
        for (k, v) in map {
            self.system_config.insert(k.clone(), v.clone());
        }
        self.save_system()
    }

    pub fn remove_system_config_key(&mut self, key: &str) -> Result<(), String> {
        self.system_config.remove(key);
        self.save_system()
    }

    pub fn set_user_config_map(&mut self, map: &Map<String, Value>) -> Result<(), String> {
        for (k, v) in map {
            self.user_config.insert(k.clone(), v.clone());
        }
        self.save_user()
    }

    pub fn reset(&mut self) -> Result<(), String> {
        self.system_config = defaults::system_defaults();
        self.user_config = defaults::user_defaults();
        self.save_system()?;
        self.save_user()?;
        Ok(())
    }

    fn migrate_legacy_keep_seeding_defaults(&mut self) -> bool {
        let Some(keep_seeding) = parse_keep_seeding_option(self.user_config.get("keep-seeding"))
        else {
            return false;
        };
        if keep_seeding {
            return false;
        }

        let seed_ratio = parse_f64_like(self.system_config.get("seed-ratio"));
        let seed_time = parse_f64_like(self.system_config.get("seed-time"));

        let is_legacy_seed_ratio = matches!(seed_ratio, Some(value) if (value - 2.0).abs() < 1e-6);
        let is_legacy_seed_time = matches!(seed_time, Some(value) if (value - 2880.0).abs() < 1e-6);

        if !is_legacy_seed_ratio || !is_legacy_seed_time {
            return false;
        }

        self.system_config.insert("seed-ratio".into(), json!(0));
        self.system_config.insert("seed-time".into(), json!(0));
        true
    }

    fn save_system(&self) -> Result<(), String> {
        let path = self.config_dir.join("system.json");
        let data = serde_json::to_string_pretty(&self.system_config).map_err(|e| e.to_string())?;
        write_atomic(&path, &data)
    }

    fn save_user(&self) -> Result<(), String> {
        let path = self.config_dir.join("user.json");
        let data = serde_json::to_string_pretty(&self.user_config).map_err(|e| e.to_string())?;
        write_atomic(&path, &data)
    }
}

/// Write `data` to `path` atomically via a sibling temp file then rename, so a crash or full disk mid-write cannot truncate/corrupt the existing file (`fs::rename` is atomic within the same directory)
fn write_atomic(path: &Path, data: &str) -> Result<(), String> {
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, data).map_err(|e| e.to_string())?;
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        e.to_string()
    })
}

fn load_or_default(path: &Path, defaults: Map<String, Value>) -> Map<String, Value> {
    match fs::read_to_string(path) {
        Ok(data) => {
            if let Ok(Value::Object(mut map)) = serde_json::from_str(&data) {
                // Fill in missing keys from defaults
                for (k, v) in &defaults {
                    if !map.contains_key(k) {
                        map.insert(k.clone(), v.clone());
                    }
                }
                return map;
            }
            // File exists but is corrupt/unparseable: back it up so the user's broken settings aren't silently overwritten, then fall back
            let backup = path.with_extension("json.bak");
            match fs::rename(path, &backup) {
                Ok(()) => tracing::warn!(
                    "Config file {} is corrupt; backed up to {} and using defaults",
                    path.display(),
                    backup.display()
                ),
                Err(err) => tracing::warn!(
                    "Config file {} is corrupt and could not be backed up ({}); using defaults",
                    path.display(),
                    err
                ),
            }
        }
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
            tracing::warn!(
                "Failed to read config {}: {}; using defaults",
                path.display(),
                err
            );
        }
        Err(_) => {}
    }
    defaults
}

pub fn parse_keep_seeding_option(value: Option<&Value>) -> Option<bool> {
    match value {
        Some(Value::Bool(v)) => Some(*v),
        Some(Value::Number(v)) => v.as_i64().map(|n| n != 0),
        Some(Value::String(v)) => {
            let normalized = v.trim().to_ascii_lowercase();
            match normalized.as_str() {
                "true" | "1" | "yes" | "on" => Some(true),
                "false" | "0" | "no" | "off" | "" => Some(false),
                _ => None,
            }
        }
        _ => None,
    }
}

fn parse_f64_like(value: Option<&Value>) -> Option<f64> {
    match value {
        Some(Value::Number(v)) => v.as_f64(),
        Some(Value::String(v)) => v.trim().parse::<f64>().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    // -- parse_keep_seeding_option --

    #[test]
    fn keep_seeding_bool() {
        assert_eq!(parse_keep_seeding_option(Some(&json!(true))), Some(true));
        assert_eq!(parse_keep_seeding_option(Some(&json!(false))), Some(false));
    }

    #[test]
    fn keep_seeding_string_truthy() {
        for s in &["true", "1", "yes", "on"] {
            assert_eq!(
                parse_keep_seeding_option(Some(&json!(s))),
                Some(true),
                "expected true for {:?}",
                s
            );
        }
    }

    // -- ConfigManager --

    #[test]
    fn config_manager_with_dir_uses_defaults_when_missing() {
        let dir = TempDir::new().unwrap();
        let mgr = ConfigManager::with_dir(dir.path().to_path_buf()).unwrap();
        // Defaults should be present when files are missing
        assert!(mgr.get_system_config().contains_key("all-proxy"));
        assert!(mgr.get_user_config().contains_key("theme"));
    }

    #[test]
    fn get_merged_config_combines_system_and_user() {
        let dir = TempDir::new().unwrap();
        let mut mgr = ConfigManager::with_dir(dir.path().to_path_buf()).unwrap();
        mgr.set_system_config_map(&serde_json::from_str(r#"{"system-key": "sys-val"}"#).unwrap())
            .unwrap();
        mgr.set_user_config_map(&serde_json::from_str(r#"{"user-key": "user-val"}"#).unwrap())
            .unwrap();

        let merged = mgr.get_merged_config();
        let map = merged.as_object().unwrap();
        assert_eq!(map.get("system-key"), Some(&json!("sys-val")));
        assert_eq!(map.get("user-key"), Some(&json!("user-val")));
        assert!(map.contains_key("platform"));
        assert!(map.contains_key("arch"));
    }

    #[test]
    fn user_config_overrides_system_config() {
        let dir = TempDir::new().unwrap();
        let mut mgr = ConfigManager::with_dir(dir.path().to_path_buf()).unwrap();
        mgr.set_system_config_map(&serde_json::from_str(r#"{"shared": "system"}"#).unwrap())
            .unwrap();
        mgr.set_user_config_map(&serde_json::from_str(r#"{"shared": "user"}"#).unwrap())
            .unwrap();

        let merged = mgr.get_merged_config();
        let map = merged.as_object().unwrap();
        assert_eq!(map.get("shared"), Some(&json!("user")));
    }

    #[test]
    fn set_system_config_map_persists() {
        let dir = TempDir::new().unwrap();
        {
            let mut mgr = ConfigManager::with_dir(dir.path().to_path_buf()).unwrap();
            mgr.set_system_config_map(&serde_json::from_str(r#"{"persisted": true}"#).unwrap())
                .unwrap();
        }
        // Re-open and verify
        let mgr2 = ConfigManager::with_dir(dir.path().to_path_buf()).unwrap();
        assert_eq!(
            mgr2.get_system_config().get("persisted"),
            Some(&json!(true))
        );
    }

    #[test]
    fn set_user_config_map_persists() {
        let dir = TempDir::new().unwrap();
        {
            let mut mgr = ConfigManager::with_dir(dir.path().to_path_buf()).unwrap();
            mgr.set_user_config_map(&serde_json::from_str(r#"{"locale": "zh-CN"}"#).unwrap())
                .unwrap();
        }
        let mgr2 = ConfigManager::with_dir(dir.path().to_path_buf()).unwrap();
        assert_eq!(mgr2.get_user_config().get("locale"), Some(&json!("zh-CN")));
    }

    #[test]
    fn remove_system_config_key() {
        let dir = TempDir::new().unwrap();
        let mut mgr = ConfigManager::with_dir(dir.path().to_path_buf()).unwrap();
        mgr.set_system_config_map(&serde_json::from_str(r#"{"a": 1, "b": 2}"#).unwrap())
            .unwrap();
        mgr.remove_system_config_key("a").unwrap();
        assert!(!mgr.get_system_config().contains_key("a"));
        assert!(mgr.get_system_config().contains_key("b"));
        // Verify persistence by reopening
        let mgr2 = ConfigManager::with_dir(dir.path().to_path_buf()).unwrap();
        assert!(!mgr2.get_system_config().contains_key("a"));
        assert!(mgr2.get_system_config().contains_key("b"));
    }

    #[test]
    fn reset_restores_defaults() {
        let dir = TempDir::new().unwrap();
        let mut mgr = ConfigManager::with_dir(dir.path().to_path_buf()).unwrap();
        mgr.set_user_config_map(&serde_json::from_str(r#"{"locale": "custom"}"#).unwrap())
            .unwrap();
        mgr.set_system_config_map(&serde_json::from_str(r#"{"dir": "/tmp"}"#).unwrap())
            .unwrap();
        mgr.reset().unwrap();

        let merged = mgr.get_merged_config();
        let map = merged.as_object().unwrap();
        assert_eq!(map.get("locale"), Some(&json!("auto")));
    }

    #[test]
    fn load_or_default_fills_missing_keys() {
        let dir = TempDir::new().unwrap();
        let user_path = dir.path().join("user.json");
        fs::write(&user_path, r#"{"locale": "fr-FR"}"#).unwrap();

        let mgr = ConfigManager::with_dir(dir.path().to_path_buf()).unwrap();
        let user = mgr.get_user_config();
        assert_eq!(user.get("locale"), Some(&json!("fr-FR")));
        // theme should be filled from defaults
        assert!(user.contains_key("theme"));
    }
}
