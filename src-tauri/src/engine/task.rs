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
}

impl DownloadTask {
    pub fn new_http(gid: String, uris: Vec<String>, dir: String, options: Map<String, Value>) -> Self {
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
        m.insert("status".into(), Value::String(self.status.as_str().to_string()));
        m.insert("totalLength".into(), Value::String(self.total_length.to_string()));
        m.insert("completedLength".into(), Value::String(self.completed_length.to_string()));
        m.insert("downloadSpeed".into(), Value::String(self.download_speed.to_string()));
        m.insert("uploadSpeed".into(), Value::String(self.upload_speed.to_string()));
        m.insert("uploadLength".into(), Value::String(self.upload_length.to_string()));
        m.insert("connections".into(), Value::String(self.connections.to_string()));
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

        m.insert("createdAt".into(), Value::String(self.created_at.to_string()));

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
            m.insert("seeder".into(), Value::String(if self.seeder { "true" } else { "false" }.into()));
            m.insert("numSeeders".into(), Value::String(self.num_seeders.to_string()));
        }

        // ed2k fields
        if self.kind == TaskKind::Ed2k {
            if let Some(uri) = self.uris.first() {
                m.insert("ed2kLink".into(), Value::String(uri.clone()));
            }
            m.insert("numPeers".into(), Value::String(self.connections.to_string()));
        }

        // m3u8 fields
        if self.kind == TaskKind::M3u8 {
            if let Some(uri) = self.uris.first() {
                m.insert("m3u8Link".into(), Value::String(uri.clone()));
            }
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
