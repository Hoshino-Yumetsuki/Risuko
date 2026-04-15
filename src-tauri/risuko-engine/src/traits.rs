use serde_json::Value;
use std::path::PathBuf;

/// Provides the directory used for app configuration and data files
///
/// - Tauri impl: uses `AppHandle::path().app_config_dir()`
/// - Standalone impl: uses `dirs::config_dir().join("risuko")`
pub trait ConfigDirProvider: Send + Sync {
    fn config_dir(&self) -> PathBuf;
}

/// Default config dir provider using the `dirs` crate
pub struct DefaultConfigDir;

impl ConfigDirProvider for DefaultConfigDir {
    fn config_dir(&self) -> PathBuf {
        dirs::config_dir()
            .map(|d| d.join("dev.risuko.app"))
            .unwrap_or_else(|| PathBuf::from("."))
    }
}

/// Receives engine events and forwards them to the host environment
///
/// - Tauri impl: calls `AppHandle::emit()` to send events to the webview
/// - NAPI impl: calls `ThreadsafeFunction` to invoke JS callbacks
/// - Standalone/CLI impl: no-op or logs
pub trait EventSink: Send + Sync {
    fn emit(&self, event: &str, payload: Value);
}

/// No-op event sink for headless/CLI usage
pub struct NoopEventSink;

impl EventSink for NoopEventSink {
    fn emit(&self, _event: &str, _payload: Value) {}
}

/// Logging event sink that prints events to the log
pub struct LogEventSink;

impl EventSink for LogEventSink {
    fn emit(&self, event: &str, payload: Value) {
        log::info!("Engine event: {} {}", event, payload);
    }
}

/// Persistent key-value storage backend for RSS data and other stores
///
/// - Tauri impl: wraps `tauri_plugin_store`
/// - File-based impl: reads/writes JSON files in the config directory
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
}

impl StorageBackend for FileStorage {
    fn load(&self, key: &str) -> Result<Option<Value>, String> {
        let path = self.dir.join(format!("{key}.json"));
        if !path.exists() {
            return Ok(None);
        }
        let data = std::fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
        let value: Value = serde_json::from_str(&data)
            .map_err(|e| format!("Failed to parse {}: {e}", path.display()))?;
        Ok(Some(value))
    }

    fn save(&self, key: &str, value: &Value) -> Result<(), String> {
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| format!("Failed to create dir {}: {e}", self.dir.display()))?;
        let path = self.dir.join(format!("{key}.json"));
        let data =
            serde_json::to_string_pretty(value).map_err(|e| format!("Failed to serialize: {e}"))?;
        std::fs::write(&path, data)
            .map_err(|e| format!("Failed to write {}: {e}", path.display()))?;
        Ok(())
    }
}
