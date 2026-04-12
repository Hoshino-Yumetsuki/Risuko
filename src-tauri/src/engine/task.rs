use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Active,
    Waiting,
    Paused,
    Complete,
    Error,
    Removed,
}

impl TaskStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Waiting => "waiting",
            Self::Paused => "paused",
            Self::Complete => "complete",
            Self::Error => "error",
            Self::Removed => "removed",
        }
    }

    pub fn is_stopped(&self) -> bool {
        matches!(self, Self::Complete | Self::Error | Self::Removed)
    }
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskKind {
    Http,
    Torrent,
    Ed2k,
    M3u8,
    Ftp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadFile {
    pub index: String,
    pub path: String,
    pub length: String,
    pub completed_length: String,
    pub selected: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub uris: Vec<FileUri>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileUri {
    pub uri: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerInfo {
    pub peer_id: String,
    pub ip: String,
    pub port: String,
    pub bitfield: String,
    pub am_choking: String,
    pub peer_choking: String,
    pub download_speed: String,
    pub upload_speed: String,
    pub seeder: String,
}

#[derive(Debug, Clone, Default)]
pub struct ChunkProgress {
    pub completed: u64,
    pub total: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadTask {
    pub gid: String,
    pub status: TaskStatus,
    pub kind: TaskKind,
    pub uris: Vec<String>,
    pub dir: String,
    pub out: String,
    pub total_length: u64,
    pub completed_length: u64,
    pub download_speed: u64,
    pub upload_speed: u64,
    #[serde(default)]
    pub upload_length: u64,
    pub connections: u32,
    pub files: Vec<DownloadFile>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub options: Map<String, Value>,
    // BitTorrent
    pub info_hash: Option<String>,
    pub bt_name: Option<String>,
    pub seeder: bool,
    pub num_seeders: u32,
    pub peers: Vec<PeerInfo>,
    // Internal tracking
    pub created_at: u64,
    /// Timestamp (ms) when seeding started, 0 if not seeding
    #[serde(default)]
    pub seeding_since: u64,
    /// split progress for multi-thread HTTP downloads (transient)
    #[serde(skip, default)]
    pub chunk_progress: Vec<ChunkProgress>,
}

impl DownloadTask {
    pub fn new_http(
        gid: String,
        uris: Vec<String>,
        dir: String,
        options: Map<String, Value>,
    ) -> Self {
        let out = options
            .get("out")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // Build initial file entry so the frontend can extract the task name from URIs
        let initial_files = if !uris.is_empty() {
            let file_uris: Vec<FileUri> = uris
                .iter()
                .map(|u| FileUri {
                    uri: u.clone(),
                    status: "waiting".to_string(),
                })
                .collect();
            // Derive initial path from output name or first URI
            // Strip .part suffix from display path so the UI shows the final name
            let display_out = out.strip_suffix(".part").unwrap_or(&out);
            let initial_path = if !display_out.is_empty() {
                format!("{}/{}", dir, display_out)
            } else {
                uris.first().cloned().unwrap_or_default()
            };
            vec![DownloadFile {
                index: "1".to_string(),
                path: initial_path,
                length: "0".to_string(),
                completed_length: "0".to_string(),
                selected: "true".to_string(),
                uris: file_uris,
            }]
        } else {
            Vec::new()
        };

        Self {
            gid,
            status: TaskStatus::Waiting,
            kind: TaskKind::Http,
            uris,
            dir,
            out,
            total_length: 0,
            completed_length: 0,
            download_speed: 0,
            upload_speed: 0,
            upload_length: 0,
            connections: 0,
            files: initial_files,
            error_code: None,
            error_message: None,
            options,
            info_hash: None,
            bt_name: None,
            seeder: false,
            num_seeders: 0,
            peers: Vec::new(),
            created_at: now_ms(),
            seeding_since: 0,
            chunk_progress: Vec::new(),
        }
    }

    pub fn new_torrent(gid: String, dir: String, options: Map<String, Value>) -> Self {
        let out = options
            .get("out")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        Self {
            gid,
            status: TaskStatus::Waiting,
            kind: TaskKind::Torrent,
            uris: Vec::new(),
            dir,
            out,
            total_length: 0,
            completed_length: 0,
            download_speed: 0,
            upload_speed: 0,
            upload_length: 0,
            connections: 0,
            files: Vec::new(),
            error_code: None,
            error_message: None,
            options,
            info_hash: None,
            bt_name: None,
            seeder: false,
            num_seeders: 0,
            peers: Vec::new(),
            created_at: now_ms(),
            seeding_since: 0,
            chunk_progress: Vec::new(),
        }
    }

    pub fn new_ed2k(
        gid: String,
        uri: String,
        file_name: String,
        file_size: u64,
        dir: String,
        options: Map<String, Value>,
    ) -> Self {
        let out = if !file_name.is_empty() {
            file_name.clone()
        } else {
            options
                .get("out")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };

        let file_path = if !out.is_empty() {
            format!("{}/{}", dir, out)
        } else {
            String::new()
        };

        let initial_files = vec![DownloadFile {
            index: "1".to_string(),
            path: file_path,
            length: file_size.to_string(),
            completed_length: "0".to_string(),
            selected: "true".to_string(),
            uris: vec![FileUri {
                uri: uri.clone(),
                status: "waiting".to_string(),
            }],
        }];

        Self {
            gid,
            status: TaskStatus::Waiting,
            kind: TaskKind::Ed2k,
            uris: vec![uri],
            dir,
            out,
            total_length: file_size,
            completed_length: 0,
            download_speed: 0,
            upload_speed: 0,
            upload_length: 0,
            connections: 0,
            files: initial_files,
            error_code: None,
            error_message: None,
            options,
            info_hash: None,
            bt_name: None,
            seeder: false,
            num_seeders: 0,
            peers: Vec::new(),
            created_at: now_ms(),
            seeding_since: 0,
            chunk_progress: Vec::new(),
        }
    }

    pub fn new_m3u8(
        gid: String,
        uri: String,
        out: String,
        dir: String,
        options: Map<String, Value>,
    ) -> Self {
        let out = if !out.is_empty() {
            out
        } else {
            options
                .get("out")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };

        let file_path = if !out.is_empty() {
            format!("{}/{}", dir, out)
        } else {
            String::new()
        };

        let initial_files = vec![DownloadFile {
            index: "1".to_string(),
            path: file_path,
            length: "0".to_string(),
            completed_length: "0".to_string(),
            selected: "true".to_string(),
            uris: vec![FileUri {
                uri: uri.clone(),
                status: "waiting".to_string(),
            }],
        }];

        Self {
            gid,
            status: TaskStatus::Waiting,
            kind: TaskKind::M3u8,
            uris: vec![uri],
            dir,
            out,
            total_length: 0,
            completed_length: 0,
            download_speed: 0,
            upload_speed: 0,
            upload_length: 0,
            connections: 0,
            files: initial_files,
            error_code: None,
            error_message: None,
            options,
            info_hash: None,
            bt_name: None,
            seeder: false,
            num_seeders: 0,
            peers: Vec::new(),
            created_at: now_ms(),
            seeding_since: 0,
            chunk_progress: Vec::new(),
        }
    }

    pub fn new_ftp(gid: String, uri: String, dir: String, options: Map<String, Value>) -> Self {
        let out = options
            .get("out")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let display_out = out.strip_suffix(".part").unwrap_or(&out);
        let initial_path = if !display_out.is_empty() {
            format!("{}/{}", dir, display_out)
        } else {
            uri.clone()
        };

        let initial_files = vec![DownloadFile {
            index: "1".to_string(),
            path: initial_path,
            length: "0".to_string(),
            completed_length: "0".to_string(),
            selected: "true".to_string(),
            uris: vec![FileUri {
                uri: uri.clone(),
                status: "waiting".to_string(),
            }],
        }];

        Self {
            gid,
            status: TaskStatus::Waiting,
            kind: TaskKind::Ftp,
            uris: vec![uri],
            dir,
            out,
            total_length: 0,
            completed_length: 0,
            download_speed: 0,
            upload_speed: 0,
            upload_length: 0,
            connections: 0,
            files: initial_files,
            error_code: None,
            error_message: None,
            options,
            info_hash: None,
            bt_name: None,
            seeder: false,
            num_seeders: 0,
            peers: Vec::new(),
            created_at: now_ms(),
            seeding_since: 0,
            chunk_progress: Vec::new(),
        }
    }

    /// Build status response for `tellStatus`
    pub fn to_rpc_status(&self, keys: &[String]) -> Value {
        let full = self.to_full_rpc_status();
        if keys.is_empty() {
            return full;
        }
        let Value::Object(map) = full else {
            return full;
        };
        let mut filtered = Map::new();
        for key in keys {
            if let Some(val) = map.get(key) {
                filtered.insert(key.clone(), val.clone());
            }
        }
        Value::Object(filtered)
    }

    fn to_full_rpc_status(&self) -> Value {
        let mut m = Map::new();
        m.insert("gid".into(), Value::String(self.gid.clone()));
        m.insert(
            "status".into(),
            Value::String(self.status.as_str().to_string()),
        );
        m.insert(
            "totalLength".into(),
            Value::String(self.total_length.to_string()),
        );
        m.insert(
            "completedLength".into(),
            Value::String(self.completed_length.to_string()),
        );
        m.insert(
            "downloadSpeed".into(),
            Value::String(self.download_speed.to_string()),
        );
        m.insert(
            "uploadSpeed".into(),
            Value::String(self.upload_speed.to_string()),
        );
        m.insert(
            "uploadLength".into(),
            Value::String(self.upload_length.to_string()),
        );
        m.insert(
            "connections".into(),
            Value::String(self.connections.to_string()),
        );
        m.insert("dir".into(), Value::String(self.dir.clone()));

        if !self.files.is_empty() {
            let files_val: Vec<Value> = self
                .files
                .iter()
                .map(|f| serde_json::to_value(f).unwrap_or(Value::Null))
                .collect();
            m.insert("files".into(), Value::Array(files_val));
        } else {
            m.insert("files".into(), Value::Array(Vec::new()));
        }

        if let Some(ref code) = self.error_code {
            m.insert("errorCode".into(), Value::String(code.clone()));
        }
        if let Some(ref msg) = self.error_message {
            m.insert("errorMessage".into(), Value::String(msg.clone()));
        }

        m.insert(
            "createdAt".into(),
            Value::String(self.created_at.to_string()),
        );

        // BitTorrent fields
        if self.kind == TaskKind::Torrent {
            let mut bt = Map::new();
            if let Some(ref hash) = self.info_hash {
                m.insert("infoHash".into(), Value::String(hash.clone()));
                bt.insert("infoHash".into(), Value::String(hash.clone()));
            }
            if let Some(ref name) = self.bt_name {
                let mut info = Map::new();
                info.insert("name".into(), Value::String(name.clone()));
                bt.insert("info".into(), Value::Object(info));
            }
            m.insert("bittorrent".into(), Value::Object(bt));
            m.insert(
                "seeder".into(),
                Value::String(if self.seeder { "true" } else { "false" }.into()),
            );
            m.insert(
                "numSeeders".into(),
                Value::String(self.num_seeders.to_string()),
            );
        }

        // ed2k fields
        if self.kind == TaskKind::Ed2k {
            if let Some(uri) = self.uris.first() {
                m.insert("ed2kLink".into(), Value::String(uri.clone()));
            }
            m.insert(
                "numPeers".into(),
                Value::String(self.connections.to_string()),
            );
        }

        // m3u8 fields
        if self.kind == TaskKind::M3u8 {
            if let Some(uri) = self.uris.first() {
                m.insert("m3u8Link".into(), Value::String(uri.clone()));
            }
        }

        // Per-chunk progress for multi-thread HTTP downloads
        if !self.chunk_progress.is_empty() {
            let chunks: Vec<Value> = self
                .chunk_progress
                .iter()
                .map(|cp| {
                    let mut cm = Map::new();
                    cm.insert(
                        "completedLength".into(),
                        Value::String(cp.completed.to_string()),
                    );
                    cm.insert("totalLength".into(), Value::String(cp.total.to_string()));
                    Value::Object(cm)
                })
                .collect();
            m.insert("chunkProgress".into(), Value::Array(chunks));
        }

        Value::Object(m)
    }
}

pub fn generate_gid() -> String {
    use rand::Rng;
    use std::fmt::Write;
    let mut rng = rand::rng();
    let bytes: [u8; 8] = rng.random();
    let mut s = String::with_capacity(16);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Map};

    // --- generate_gid ---

    #[test]
    fn gid_is_16_hex_chars() {
        let gid = generate_gid();
        assert_eq!(gid.len(), 16);
        assert!(gid.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn gid_is_unique() {
        let a = generate_gid();
        let b = generate_gid();
        assert_ne!(a, b);
    }

    // --- TaskStatus ---

    #[test]
    fn status_as_str() {
        assert_eq!(TaskStatus::Active.as_str(), "active");
        assert_eq!(TaskStatus::Waiting.as_str(), "waiting");
        assert_eq!(TaskStatus::Paused.as_str(), "paused");
        assert_eq!(TaskStatus::Complete.as_str(), "complete");
        assert_eq!(TaskStatus::Error.as_str(), "error");
        assert_eq!(TaskStatus::Removed.as_str(), "removed");
    }

    #[test]
    fn status_is_stopped() {
        assert!(TaskStatus::Complete.is_stopped());
        assert!(TaskStatus::Error.is_stopped());
        assert!(TaskStatus::Removed.is_stopped());
        assert!(!TaskStatus::Active.is_stopped());
        assert!(!TaskStatus::Waiting.is_stopped());
        assert!(!TaskStatus::Paused.is_stopped());
    }

    #[test]
    fn status_display() {
        assert_eq!(format!("{}", TaskStatus::Active), "active");
        assert_eq!(format!("{}", TaskStatus::Complete), "complete");
    }

    // --- DownloadTask constructors ---

    #[test]
    fn new_http_basic() {
        let opts = Map::new();
        let uris = vec!["http://example.com/file.zip".to_string()];
        let task = DownloadTask::new_http("gid1".into(), uris.clone(), "/tmp".into(), opts);

        assert_eq!(task.gid, "gid1");
        assert_eq!(task.status, TaskStatus::Waiting);
        assert_eq!(task.kind, TaskKind::Http);
        assert_eq!(task.uris, uris);
        assert_eq!(task.dir, "/tmp");
        assert_eq!(task.total_length, 0);
        assert_eq!(task.files.len(), 1);
        assert_eq!(task.files[0].uris.len(), 1);
    }

    #[test]
    fn new_http_strips_part_from_display_path() {
        let mut opts = Map::new();
        opts.insert("out".into(), json!("file.zip.part"));
        let uris = vec!["http://example.com/file.zip".to_string()];
        let task = DownloadTask::new_http("gid1".into(), uris, "/dl".into(), opts);

        assert_eq!(task.out, "file.zip.part");
        // Display path should have .part stripped
        assert_eq!(task.files[0].path, "/dl/file.zip");
    }

    #[test]
    fn new_torrent_basic() {
        let opts = Map::new();
        let task = DownloadTask::new_torrent("gid2".into(), "/dl".into(), opts);

        assert_eq!(task.kind, TaskKind::Torrent);
        assert_eq!(task.status, TaskStatus::Waiting);
        assert!(task.files.is_empty());
        assert!(task.info_hash.is_none());
    }

    #[test]
    fn new_ed2k_sets_file_size() {
        let opts = Map::new();
        let task = DownloadTask::new_ed2k(
            "gid3".into(),
            "ed2k://|file|test.bin|1024|hash|/".into(),
            "test.bin".into(),
            1024,
            "/dl".into(),
            opts,
        );

        assert_eq!(task.kind, TaskKind::Ed2k);
        assert_eq!(task.total_length, 1024);
        assert_eq!(task.out, "test.bin");
        assert_eq!(task.files[0].length, "1024");
        assert_eq!(task.files[0].path, "/dl/test.bin");
    }

    #[test]
    fn new_m3u8_basic() {
        let opts = Map::new();
        let task = DownloadTask::new_m3u8(
            "gid4".into(),
            "http://example.com/stream.m3u8".into(),
            "stream.ts".into(),
            "/dl".into(),
            opts,
        );

        assert_eq!(task.kind, TaskKind::M3u8);
        assert_eq!(task.out, "stream.ts");
        assert_eq!(task.files[0].path, "/dl/stream.ts");
    }

    #[test]
    fn new_ftp_basic() {
        let opts = Map::new();
        let task = DownloadTask::new_ftp(
            "gid5".into(),
            "ftp://files.example.com/data.csv".into(),
            "/dl".into(),
            opts,
        );

        assert_eq!(task.kind, TaskKind::Ftp);
        assert_eq!(task.status, TaskStatus::Waiting);
        assert_eq!(task.uris[0], "ftp://files.example.com/data.csv");
    }

    // --- to_rpc_status ---

    #[test]
    fn rpc_status_all_keys() {
        let task = DownloadTask::new_http(
            "test_gid".into(),
            vec!["http://example.com/f.bin".into()],
            "/tmp".into(),
            Map::new(),
        );
        let status = task.to_rpc_status(&[]);
        let obj = status.as_object().unwrap();

        assert_eq!(obj.get("gid").unwrap(), "test_gid");
        assert_eq!(obj.get("status").unwrap(), "waiting");
        assert!(obj.contains_key("totalLength"));
        assert!(obj.contains_key("files"));
        assert!(obj.contains_key("dir"));
    }

    #[test]
    fn rpc_status_filtered_keys() {
        let task = DownloadTask::new_http(
            "test_gid".into(),
            vec!["http://example.com/f.bin".into()],
            "/tmp".into(),
            Map::new(),
        );
        let keys = vec!["gid".to_string(), "status".to_string()];
        let status = task.to_rpc_status(&keys);
        let obj = status.as_object().unwrap();

        assert_eq!(obj.len(), 2);
        assert!(obj.contains_key("gid"));
        assert!(obj.contains_key("status"));
    }

    #[test]
    fn rpc_status_torrent_has_bittorrent_field() {
        let mut task = DownloadTask::new_torrent("tgid".into(), "/dl".into(), Map::new());
        task.info_hash = Some("abc123".into());
        task.bt_name = Some("My Torrent".into());

        let status = task.to_rpc_status(&[]);
        let obj = status.as_object().unwrap();

        assert!(obj.contains_key("bittorrent"));
        assert!(obj.contains_key("seeder"));
        let bt = obj.get("bittorrent").unwrap().as_object().unwrap();
        assert_eq!(bt.get("infoHash").unwrap(), "abc123");
    }

    #[test]
    fn rpc_status_ed2k_has_ed2k_link() {
        let task = DownloadTask::new_ed2k(
            "egid".into(),
            "ed2k://|file|test|100|hash|/".into(),
            "test".into(),
            100,
            "/dl".into(),
            Map::new(),
        );
        let status = task.to_rpc_status(&[]);
        let obj = status.as_object().unwrap();
        assert!(obj.contains_key("ed2kLink"));
    }
}
