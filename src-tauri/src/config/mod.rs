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
                log::warn!(
                    "Failed to persist legacy keep-seeding migration; continuing startup: {}",
                    err
                );
            }
        }

        Ok(manager)
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

        // Add runtime context
        merged.insert("platform".into(), json!(std::env::consts::OS));
        merged.insert("arch".into(), json!(std::env::consts::ARCH));

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
        fs::write(path, data).map_err(|e| e.to_string())?;
        Ok(())
    }

    fn save_user(&self) -> Result<(), String> {
        let path = self.config_dir.join("user.json");
        let data = serde_json::to_string_pretty(&self.user_config).map_err(|e| e.to_string())?;
        fs::write(path, data).map_err(|e| e.to_string())?;
        Ok(())
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
            // Fill in missing keys from defaults
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

pub(crate) fn parse_keep_seeding_option(value: Option<&Value>) -> Option<bool> {
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

    #[test]
    fn keep_seeding_string_falsy() {
        for s in &["false", "0", "no", "off", ""] {
            assert_eq!(
                parse_keep_seeding_option(Some(&json!(s))),
                Some(false),
                "expected false for {:?}",
                s
            );
        }
    }

    #[test]
    fn keep_seeding_number() {
        assert_eq!(parse_keep_seeding_option(Some(&json!(1))), Some(true));
        assert_eq!(parse_keep_seeding_option(Some(&json!(0))), Some(false));
        assert_eq!(parse_keep_seeding_option(Some(&json!(-1))), Some(true));
    }

    #[test]
    fn keep_seeding_none_and_unknown() {
        assert_eq!(parse_keep_seeding_option(None), None);
        assert_eq!(parse_keep_seeding_option(Some(&json!("random"))), None);
    }

    // -- parse_f64_like --

    #[test]
    fn f64_like_number() {
        assert_eq!(parse_f64_like(Some(&json!(3.14))), Some(3.14));
        assert_eq!(parse_f64_like(Some(&json!(0))), Some(0.0));
    }

    #[test]
    fn f64_like_string() {
        assert_eq!(parse_f64_like(Some(&json!("2.5"))), Some(2.5));
        assert_eq!(parse_f64_like(Some(&json!(" 4.0 "))), Some(4.0));
    }

    #[test]
    fn f64_like_invalid() {
        assert_eq!(parse_f64_like(Some(&json!("abc"))), None);
        assert_eq!(parse_f64_like(None), None);
        assert_eq!(parse_f64_like(Some(&json!(null))), None);
    }

    // -- load_or_default --

    #[test]
    fn load_or_default_missing_file_returns_defaults() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("does_not_exist.json");
        let mut defaults = Map::new();
        defaults.insert("key".into(), json!("val"));
        let result = load_or_default(&path, defaults.clone());
        assert_eq!(result, defaults);
    }

    #[test]
    fn load_or_default_corrupt_json_returns_defaults() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, "not json!!!").unwrap();

        let mut defaults = Map::new();
        defaults.insert("k".into(), json!(1));
        let result = load_or_default(&path, defaults.clone());
        assert_eq!(result, defaults);
    }

    #[test]
    fn load_or_default_valid_file_fills_missing_keys() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, r#"{"existing": "value"}"#).unwrap();

        let mut defaults = Map::new();
        defaults.insert("existing".into(), json!("default"));
        defaults.insert("missing".into(), json!("filled"));

        let result = load_or_default(&path, defaults);
        assert_eq!(result.get("existing").unwrap(), "value"); // preserved from file
        assert_eq!(result.get("missing").unwrap(), "filled"); // filled from defaults
    }

    // -- get_merged_config --

    #[test]
    fn merged_config_combines_system_and_user() {
        let mut sys = Map::new();
        sys.insert("dir".into(), json!("/downloads"));
        sys.insert("split".into(), json!(16));

        let mut user = Map::new();
        user.insert("theme".into(), json!("dark"));
        user.insert("dir".into(), json!("/user-dir")); // user overrides system

        let mgr = ConfigManager {
            system_config: sys,
            user_config: user,
            config_dir: PathBuf::from("/tmp"),
        };

        let merged = mgr.get_merged_config();
        let obj = merged.as_object().unwrap();

        assert_eq!(obj.get("dir").unwrap(), "/user-dir"); // user wins
        assert_eq!(obj.get("split").unwrap(), 16);
        assert_eq!(obj.get("theme").unwrap(), "dark");
        assert!(obj.contains_key("platform"));
        assert!(obj.contains_key("arch"));
    }

    // -- migrate_legacy_keep_seeding_defaults --

    #[test]
    fn migration_resets_legacy_seed_values() {
        let mut sys = Map::new();
        sys.insert("seed-ratio".into(), json!(2.0));
        sys.insert("seed-time".into(), json!(2880.0));

        let mut user = Map::new();
        user.insert("keep-seeding".into(), json!(false));

        let mut mgr = ConfigManager {
            system_config: sys,
            user_config: user,
            config_dir: PathBuf::from("/tmp"),
        };

        let migrated = mgr.migrate_legacy_keep_seeding_defaults();
        assert!(migrated);
        assert_eq!(mgr.system_config.get("seed-ratio").unwrap(), 0);
        assert_eq!(mgr.system_config.get("seed-time").unwrap(), 0);
    }

    #[test]
    fn migration_skips_when_keep_seeding_true() {
        let mut sys = Map::new();
        sys.insert("seed-ratio".into(), json!(2.0));
        sys.insert("seed-time".into(), json!(2880.0));

        let mut user = Map::new();
        user.insert("keep-seeding".into(), json!(true));

        let mut mgr = ConfigManager {
            system_config: sys,
            user_config: user,
            config_dir: PathBuf::from("/tmp"),
        };

        assert!(!mgr.migrate_legacy_keep_seeding_defaults());
    }

    #[test]
    fn migration_skips_when_non_legacy_values() {
        let mut sys = Map::new();
        sys.insert("seed-ratio".into(), json!(1.0));
        sys.insert("seed-time".into(), json!(60.0));

        let mut user = Map::new();
        user.insert("keep-seeding".into(), json!(false));

        let mut mgr = ConfigManager {
            system_config: sys,
            user_config: user,
            config_dir: PathBuf::from("/tmp"),
        };

        assert!(!mgr.migrate_legacy_keep_seeding_defaults());
    }
}
