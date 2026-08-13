use serde_json::Value;
use std::path::PathBuf;

/// Provides the directory used for app configuration and data files - Tauri impl: uses `AppHandle::path().app_config_dir()` - Standalone impl: uses `dirs::config_dir().join("dev.risuko.app")`
pub trait ConfigDirProvider: Send + Sync {
    fn config_dir(&self) -> PathBuf;
}

/// Receives engine events and forwards them to the host environment - Tauri impl: calls `AppHandle::emit()` to send events to the webview - NAPI impl: calls `ThreadsafeFunction` to invoke JS callbacks - Standalone/CLI impl: no-op or logs
pub trait EventSink: Send + Sync {
    fn emit(&self, event: &str, payload: Value);
}

/// No-op event sink for headless/CLI usage
pub struct NoopEventSink;

impl EventSink for NoopEventSink {
    fn emit(&self, _event: &str, _payload: Value) {}
}

/// Persistent key-value storage backend for RSS data and other stores - Tauri impl: wraps `tauri_plugin_store` - File-based impl: reads/writes JSON files in the config directory
pub trait StorageBackend: Send + Sync {
    fn load(&self, key: &str) -> Result<Option<Value>, String>;
    fn save(&self, key: &str, value: &Value) -> Result<(), String>;
}

/// File-based storage backend that persists JSON data in the config directory
pub struct FileStorage {
    dir: PathBuf,
}

impl FileStorage {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// Reject keys that could escape the storage directory (path separators, parent refs, absolute/rooted paths). Keys map to `<dir>/<key>.json`, so a key like `../foo` or `a/b` must not be allowed to traverse outside
    fn safe_path(&self, key: &str) -> Result<PathBuf, String> {
        if key.is_empty()
            || key == "."
            || key == ".."
            || key.contains('/')
            || key.contains('\\')
            || key.contains('\0')
            || std::path::Path::new(key)
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return Err(format!("invalid storage key: {key:?}"));
        }
        Ok(self.dir.join(format!("{key}.json")))
    }
}

impl StorageBackend for FileStorage {
    fn load(&self, key: &str) -> Result<Option<Value>, String> {
        let path = self.safe_path(key)?;
        // Read directly and treat NotFound as "no value" to avoid a TOCTOU gap between an exists() check and the read
        let data = match std::fs::read_to_string(&path) {
            Ok(data) => data,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("Failed to read {}: {e}", path.display())),
        };
        let value: Value = serde_json::from_str(&data)
            .map_err(|e| format!("Failed to parse {}: {e}", path.display()))?;
        Ok(Some(value))
    }

    fn save(&self, key: &str, value: &Value) -> Result<(), String> {
        let path = self.safe_path(key)?;
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| format!("Failed to create dir {}: {e}", self.dir.display()))?;
        let data =
            serde_json::to_string_pretty(value).map_err(|e| format!("Failed to serialize: {e}"))?;
        // Atomic write: temp file + rename so a crash mid-write can't corrupt the existing persisted JSON
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, data)
            .map_err(|e| format!("Failed to write {}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("Failed to persist {}: {e}", path.display())
        })?;
        Ok(())
    }
}
