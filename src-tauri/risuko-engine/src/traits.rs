use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};

pub(crate) fn write_file_atomically(path: &Path, data: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("Failed to create dir {}: {e}", parent.display()))?;

    let mut temp = tempfile::NamedTempFile::new_in(parent)
        .map_err(|e| format!("Failed to create temp file in {}: {e}", parent.display()))?;
    temp.as_file_mut()
        .write_all(data)
        .map_err(|e| format!("Failed to write temporary file for {}: {e}", path.display()))?;
    temp.as_file()
        .sync_all()
        .map_err(|e| format!("Failed to sync temporary file for {}: {e}", path.display()))?;
    temp.persist(path)
        .map_err(|e| format!("Failed to persist {}: {}", path.display(), e.error))?;

    // Best effort: the file itself is durable and atomically replaced even on
    // platforms that do not allow opening a directory for syncing.
    if let Ok(directory) = std::fs::File::open(parent) {
        let _ = directory.sync_all();
    }
    Ok(())
}

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
        let data =
            serde_json::to_string_pretty(value).map_err(|e| format!("Failed to serialize: {e}"))?;
        write_file_atomically(&path, data.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::{Arc, Barrier};

    #[test]
    fn concurrent_saves_of_one_key_use_independent_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Arc::new(FileStorage::new(dir.path().to_path_buf()));
        let barrier = Arc::new(Barrier::new(3));

        let handles: Vec<_> = [json!({"writer": 1}), json!({"writer": 2})]
            .into_iter()
            .map(|value| {
                let storage = Arc::clone(&storage);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    storage.save("shared", &value)
                })
            })
            .collect();
        barrier.wait();

        for handle in handles {
            handle.join().unwrap().unwrap();
        }
        let saved = storage.load("shared").unwrap().unwrap();
        assert!(saved == json!({"writer": 1}) || saved == json!({"writer": 2}));
    }
}
