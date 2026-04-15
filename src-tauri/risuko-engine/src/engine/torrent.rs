use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// BitTorrent download management via librqbit
///
/// librqbit provides a `Session` that manages all torrent downloads
/// We wrap it to integrate with our task management system

pub struct TorrentEngine {
    session: Option<Arc<librqbit::Session>>,
    output_dir: PathBuf,
}

impl TorrentEngine {
    pub async fn new(output_dir: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(output_dir)
            .map_err(|e| format!("Failed to create torrent output dir: {}", e))?;

        let session = librqbit::Session::new_with_opts(
            output_dir.to_path_buf(),
            librqbit::SessionOptions {
                disable_dht: false,
                disable_dht_persistence: false,
                dht_config: None,
                listen_port_range: Some(21301..21400),
                enable_upnp_port_forwarding: true,
                fastresume: true,
                persistence: Some(librqbit::SessionPersistenceConfig::Json { folder: None }),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| format!("Failed to create torrent session: {}", e))?;

        log::info!(
            "Torrent engine initialized, output_dir={}",
            output_dir.display()
        );

        Ok(Self {
            session: Some(session),
            output_dir: output_dir.to_path_buf(),
        })
    }

    fn get_session(&self) -> Result<&Arc<librqbit::Session>, String> {
        self.session
            .as_ref()
            .ok_or_else(|| "Torrent engine not initialized".to_string())
    }

    /// List all torrents currently managed by the session
    /// Returns (librqbit_id, info_hash_string) pairs
    pub fn list_managed_torrents(&self) -> Vec<(usize, String)> {
        let Some(session) = self.session.as_ref() else {
            return Vec::new();
        };
        session.with_torrents(|iter| {
            iter.map(|(id, handle)| {
                let hash = handle.info_hash().as_string();
                (id, hash)
            })
            .collect()
        })
    }

    /// Parse `select-file` option into a list of 0-based file indices for librqbit
    /// The input uses aria2-compatible 1-based indices, e.g. "1,2,5"
    fn parse_select_files(options: &Map<String, Value>) -> Option<Vec<usize>> {
        let raw = options.get("select-file").and_then(|v| v.as_str())?.trim();
        if raw.is_empty() {
            return None;
        }

        let indices: Vec<usize> = raw
            .split(',')
            .filter_map(|s| {
                let s = s.trim();
                if s.is_empty() {
                    return None;
                }
                s.parse::<usize>().ok().and_then(|i| {
                    if i >= 1 {
                        Some(i - 1)
                    } else {
                        None
                    } // Convert 1-based to 0-based
                })
            })
            .collect();

        if indices.is_empty() {
            None
        } else {
            Some(indices)
        }
    }

    /// Add a torrent from a .torrent file's bytes
    pub async fn add_torrent_bytes(
        &self,
        data: &[u8],
        options: &Map<String, Value>,
    ) -> Result<TorrentHandle, String> {
        let session = self.get_session()?;

        let dir = options
            .get("dir")
            .and_then(|v| v.as_str())
            .unwrap_or(self.output_dir.to_str().unwrap_or("."));

        let trackers = Self::parse_trackers(options);
        let only_files = Self::parse_select_files(options);

        let add_opts = librqbit::AddTorrentOptions {
            output_folder: Some(dir.to_string()),
            overwrite: true,
            trackers: if trackers.is_empty() {
                None
            } else {
                Some(trackers)
            },
            only_files,
            ..Default::default()
        };

        log::info!("Adding torrent bytes ({} bytes) to dir={}", data.len(), dir);

        let response = session
            .add_torrent(
                librqbit::AddTorrent::TorrentFileBytes(data.to_vec().into()),
                Some(add_opts),
            )
            .await
            .map_err(|e| format!("Failed to add torrent: {}", e))?;

        let handle = extract_handle(response)?;
        log::info!(
            "Torrent added: id={}, info_hash={:?}",
            handle.id,
            handle.info_hash
        );
        Ok(handle)
    }

    /// Add a torrent from a magnet link
    pub async fn add_magnet(
        &self,
        magnet_uri: &str,
        options: &Map<String, Value>,
    ) -> Result<TorrentHandle, String> {
        let session = self.get_session()?;

        let dir = options
            .get("dir")
            .and_then(|v| v.as_str())
            .unwrap_or(self.output_dir.to_str().unwrap_or("."));

        let trackers = Self::parse_trackers(options);
        let only_files = Self::parse_select_files(options);

        let add_opts = librqbit::AddTorrentOptions {
            output_folder: Some(dir.to_string()),
            overwrite: true,
            trackers: if trackers.is_empty() {
                None
            } else {
                Some(trackers)
            },
            only_files,
            ..Default::default()
        };

        log::info!("Adding magnet to dir={}: {}", dir, magnet_uri);

        let response = session
            .add_torrent(librqbit::AddTorrent::Url(magnet_uri.into()), Some(add_opts))
            .await
            .map_err(|e| format!("Failed to add magnet: {}", e))?;

        let handle = extract_handle(response)?;
        log::info!(
            "Magnet added: id={}, info_hash={:?}",
            handle.id,
            handle.info_hash
        );
        Ok(handle)
    }

    /// Parse user's bt-tracker config into a list of tracker URLs
    fn parse_trackers(options: &Map<String, Value>) -> Vec<String> {
        options
            .get("bt-tracker")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }

    /// Get the status of a torrent by its session ID
    pub fn get_torrent_stats(&self, torrent_id: usize) -> Option<TorrentStats> {
        let session = self.session.as_ref()?;
        let handle = session.get(librqbit::api::TorrentIdOrHash::Id(torrent_id))?;
        let stats = handle.stats();

        let (download_speed, upload_speed, num_peers) = match &stats.live {
            Some(live) => {
                let peers = live.snapshot.peer_stats.live as u32;
                let dl = (live.download_speed.mbps * 1_048_576.0) as u64;
                let ul = (live.upload_speed.mbps * 1_048_576.0) as u64;
                (dl, ul, peers)
            }
            None => (0, 0, 0),
        };

        let name = handle.name();

        // Collect per-file information from metadata
        let file_details = handle.metadata.load().as_ref().and_then(|r| {
            r.info.iter_file_details().ok().map(|iter| {
                iter.enumerate()
                    .map(|(idx, d)| {
                        let filename = d
                            .filename
                            .to_string()
                            .unwrap_or_else(|_| format!("file_{}", idx));
                        TorrentFileInfo {
                            index: idx,
                            path: filename,
                            length: d.len,
                        }
                    })
                    .collect::<Vec<_>>()
            })
        });

        Some(TorrentStats {
            total_bytes: stats.total_bytes,
            downloaded_bytes: stats.progress_bytes,
            uploaded_bytes: stats.uploaded_bytes,
            download_speed,
            upload_speed,
            num_peers,
            is_finished: stats.finished,
            name,
            file_progress: stats.file_progress,
            file_details,
        })
    }

    /// Pause a torrent
    pub async fn pause(&self, torrent_id: usize) -> Result<(), String> {
        let session = self.get_session()?;
        let handle = session
            .get(librqbit::api::TorrentIdOrHash::Id(torrent_id))
            .ok_or("Torrent not found")?;
        session
            .pause(&handle)
            .await
            .map_err(|e| format!("Failed to pause: {}", e))
    }

    /// Unpause a torrent
    pub async fn unpause(&self, torrent_id: usize) -> Result<(), String> {
        let session = self.get_session()?;
        let handle = session
            .get(librqbit::api::TorrentIdOrHash::Id(torrent_id))
            .ok_or("Torrent not found")?;
        session
            .unpause(&handle)
            .await
            .map_err(|e| format!("Failed to unpause: {}", e))
    }

    /// Remove a torrent
    pub async fn remove(&self, torrent_id: usize) -> Result<(), String> {
        let session = self.get_session()?;
        session
            .delete(librqbit::api::TorrentIdOrHash::Id(torrent_id), false)
            .await
            .map_err(|e| format!("Failed to remove torrent: {}", e))
    }

    pub async fn shutdown(&mut self) {
        if let Some(session) = self.session.take() {
            drop(session);
        }
    }
}

fn extract_handle(response: librqbit::AddTorrentResponse) -> Result<TorrentHandle, String> {
    match response {
        librqbit::AddTorrentResponse::Added(id, handle) => Ok(TorrentHandle {
            id,
            info_hash: Some(handle.info_hash().as_string()),
        }),
        librqbit::AddTorrentResponse::AlreadyManaged(id, handle) => Ok(TorrentHandle {
            id,
            info_hash: Some(handle.info_hash().as_string()),
        }),
        librqbit::AddTorrentResponse::ListOnly(_) => {
            Err("Torrent was added in list-only mode".to_string())
        }
    }
}

pub struct TorrentHandle {
    pub id: usize,
    pub info_hash: Option<String>,
}

pub struct TorrentFileInfo {
    pub index: usize,
    pub path: String,
    pub length: u64,
}

pub struct TorrentStats {
    pub total_bytes: u64,
    pub downloaded_bytes: u64,
    pub uploaded_bytes: u64,
    pub download_speed: u64,
    pub upload_speed: u64,
    pub num_peers: u32,
    pub is_finished: bool,
    pub name: Option<String>,
    pub file_progress: Vec<u64>,
    pub file_details: Option<Vec<TorrentFileInfo>>,
}

/// Check if a URI is a magnet link
pub fn is_magnet_uri(uri: &str) -> bool {
    uri.trim().to_lowercase().starts_with("magnet:")
}
