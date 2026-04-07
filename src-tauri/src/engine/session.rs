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

impl SessionData {
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self {
            version: 1,
            tasks: Vec::new(),
        }
    }
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
        fs::rename(&tmp, &self.path)
            .map_err(|e| format!("Failed to finalize session: {}", e))?;

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
