use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

use super::task::{DownloadTask, TaskStatus};

pub const SESSION_FILENAME: &str = "engine-session.json";

/// JSON-based session persistence
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionData {
    pub version: u32,
    pub tasks: Vec<DownloadTask>,
}

pub struct SessionManager {
    path: PathBuf,
}

impl SessionManager {
    pub fn new(config_dir: &Path) -> Self {
        let path = config_dir.join(SESSION_FILENAME);
        Self { path }
    }

    /// Load persisted tasks. Returns empty vec on missing/corrupt file
    pub fn load(&self) -> Vec<DownloadTask> {
        let data = match fs::read_to_string(&self.path) {
            Ok(d) => d,
            Err(_) => return Vec::new(),
        };

        match serde_json::from_str::<SessionData>(&data) {
            Ok(session) => {
                // Restore all tasks except explicitly removed ones
                session
                    .tasks
                    .into_iter()
                    .filter(|t| !matches!(t.status, TaskStatus::Removed))
                    .map(|mut t| {
                        // Reset runtime state
                        if t.status == TaskStatus::Active {
                            t.status = TaskStatus::Paused;
                        }
                        t.download_speed = 0;
                        t.upload_speed = 0;
                        t.connections = 0;
                        t
                    })
                    .collect()
            }
            Err(e) => {
                log::warn!("Failed to parse engine session: {}", e);
                Vec::new()
            }
        }
    }

    /// Save current tasks to disk
    pub fn save(&self, tasks: &[DownloadTask]) -> Result<(), String> {
        let session = SessionData {
            version: 1,
            tasks: tasks
                .iter()
                .filter(|t| !matches!(t.status, TaskStatus::Removed))
                .cloned()
                .collect(),
        };

        let data = serde_json::to_string_pretty(&session).map_err(|e| e.to_string())?;

        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }

        // Atomic write: write to temp file, then rename
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, &data).map_err(|e| format!("Failed to write session: {}", e))?;
        fs::rename(&tmp, &self.path).map_err(|e| format!("Failed to finalize session: {}", e))?;

        Ok(())
    }

    /// Delete old aria2 session file if present
    pub fn cleanup_legacy(config_dir: &Path) {
        let legacy = config_dir.join("download.session");
        if legacy.exists() {
            let _ = fs::remove_file(&legacy);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Map;
    use tempfile::TempDir;

    fn make_http_task(gid: &str, status: TaskStatus) -> DownloadTask {
        let mut task = DownloadTask::new_http(
            gid.into(),
            vec!["http://example.com/file.zip".into()],
            "/dl".into(),
            Map::new(),
        );
        task.status = status;
        task
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = TempDir::new().unwrap();
        let mgr = SessionManager::new(dir.path());

        let tasks = vec![
            make_http_task("gid1", TaskStatus::Paused),
            make_http_task("gid2", TaskStatus::Complete),
        ];

        mgr.save(&tasks).unwrap();
        let loaded = mgr.load();

        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].gid, "gid1");
        assert_eq!(loaded[0].status, TaskStatus::Paused);
        assert_eq!(loaded[1].gid, "gid2");
        assert_eq!(loaded[1].status, TaskStatus::Complete);
    }

    #[test]
    fn load_missing_file_returns_empty() {
        let dir = TempDir::new().unwrap();
        let mgr = SessionManager::new(dir.path());
        assert!(mgr.load().is_empty());
    }

    #[test]
    fn load_corrupt_json_returns_empty() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(SESSION_FILENAME);
        fs::write(&path, "not valid json {{{").unwrap();

        let mgr = SessionManager::new(dir.path());
        assert!(mgr.load().is_empty());
    }

    #[test]
    fn active_tasks_become_paused_on_load() {
        let dir = TempDir::new().unwrap();
        let mgr = SessionManager::new(dir.path());

        let mut task = make_http_task("gid1", TaskStatus::Active);
        task.download_speed = 1000;
        task.connections = 5;

        mgr.save(&[task]).unwrap();
        let loaded = mgr.load();

        assert_eq!(loaded[0].status, TaskStatus::Paused);
        assert_eq!(loaded[0].download_speed, 0);
        assert_eq!(loaded[0].connections, 0);
    }

    #[test]
    fn removed_tasks_filtered_on_save_and_load() {
        let dir = TempDir::new().unwrap();
        let mgr = SessionManager::new(dir.path());

        let tasks = vec![
            make_http_task("kept", TaskStatus::Paused),
            make_http_task("gone", TaskStatus::Removed),
        ];

        mgr.save(&tasks).unwrap();
        let loaded = mgr.load();

        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].gid, "kept");
    }

    #[test]
    fn cleanup_legacy_removes_old_session() {
        let dir = TempDir::new().unwrap();
        let legacy = dir.path().join("download.session");
        fs::write(&legacy, "old data").unwrap();
        assert!(legacy.exists());

        SessionManager::cleanup_legacy(dir.path());
        assert!(!legacy.exists());
    }

    #[test]
    fn cleanup_legacy_no_op_when_missing() {
        let dir = TempDir::new().unwrap();
        // Should not panic
        SessionManager::cleanup_legacy(dir.path());
    }
}
