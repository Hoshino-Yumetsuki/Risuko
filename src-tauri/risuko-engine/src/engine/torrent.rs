use risuko_bt as bt;
use serde_json::{Map, Value};
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// BitTorrent download management via the in-tree `risuko-bt` engine.
pub struct TorrentEngine {
    session: Option<Arc<bt::Session>>,
    output_dir: PathBuf,
}

impl TorrentEngine {
    pub async fn new(output_dir: &Path) -> Result<Self, String> {
        Self::new_with_tuning(output_dir, None, None).await
    }

    pub async fn new_with_tuning(
        output_dir: &Path,
        max_outstanding_per_peer: Option<usize>,
        max_peers_per_torrent: Option<usize>,
    ) -> Result<Self, String> {
        std::fs::create_dir_all(output_dir)
            .map_err(|e| format!("Failed to create torrent output dir: {}", e))?;

        let session = bt::Session::new_with_opts(
            output_dir.to_path_buf(),
            bt::SessionOptions {
                listen: Some(bt::ListenerOptions {
                    listen_addr: Some((Ipv4Addr::UNSPECIFIED, 0).into()),
                    enable_upnp_port_forwarding: true,
                }),
                fastresume: true,
                max_outstanding_requests_per_peer: max_outstanding_per_peer,
                max_peers_per_torrent,
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

    fn get_session(&self) -> Result<&Arc<bt::Session>, String> {
        self.session
            .as_ref()
            .ok_or_else(|| "Torrent engine not initialized".to_string())
    }

    pub fn list_managed_torrents(&self) -> Vec<(usize, String)> {
        let Some(session) = self.session.as_ref() else {
            return Vec::new();
        };
        session.with_torrents(|iter| {
            iter.map(|(id, handle)| (id, handle.info_hash().as_string()))
                .collect()
        })
    }

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
                s.parse::<usize>()
                    .ok()
                    .and_then(|i| if i >= 1 { Some(i - 1) } else { None })
            })
            .collect();
        if indices.is_empty() {
            None
        } else {
            Some(indices)
        }
    }

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

        let add_opts = bt::AddTorrentOptions {
            output_folder: Some(dir.to_string()),
            overwrite: true,
            trackers: if trackers.is_empty() {
                None
            } else {
                Some(trackers)
            },
            only_files,
            list_only: false,
        };

        log::info!("Adding torrent bytes ({} bytes) to dir={}", data.len(), dir);

        let response = session
            .add_torrent(
                bt::AddTorrent::TorrentFileBytes(data.to_vec().into()),
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

        let add_opts = bt::AddTorrentOptions {
            output_folder: Some(dir.to_string()),
            overwrite: true,
            trackers: if trackers.is_empty() {
                None
            } else {
                Some(trackers.clone())
            },
            only_files,
            list_only: false,
        };

        log::info!("Adding magnet to dir={}: {}", dir, magnet_uri);

        let save_metadata = options
            .get("bt-save-metadata")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // When bt-save-metadata is enabled, resolve the magnet ourselves first
        // so we can write the synthesized .torrent file next to the payload.
        // The resulting bytes are reused to add the torrent, so we do not pay
        // the resolve cost twice.
        if save_metadata {
            match bt::magnet::resolve(magnet_uri, &trackers, Duration::from_secs(120)).await {
                Ok(resolved) => {
                    let bytes =
                        bt::magnet::synth_torrent_bytes(&resolved.info_bytes, &resolved.trackers);
                    if let Ok(meta) = bt::parse_torrent(&bytes) {
                        let name = if meta.info.name.is_empty() {
                            format!("{:?}", meta.info_hash)
                        } else {
                            meta.info.name.clone()
                        };
                        let safe = sanitize_file_stem(&name);
                        let path = Path::new(dir).join(format!("{}.torrent", safe));
                        match tokio::fs::write(&path, &bytes).await {
                            Ok(()) => log::info!("Saved torrent metadata to {}", path.display()),
                            Err(e) => log::warn!(
                                "Failed to save torrent metadata to {}: {}",
                                path.display(),
                                e
                            ),
                        }
                    }
                    return self.add_torrent_bytes(&bytes, options).await;
                }
                Err(e) => {
                    log::warn!("bt-save-metadata resolve failed, falling back: {}", e);
                }
            }
        }

        let response = session
            .add_torrent(bt::AddTorrent::Url(magnet_uri.into()), Some(add_opts))
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

    pub async fn resolve_magnet(
        &self,
        magnet_uri: &str,
        options: &Map<String, Value>,
        timeout_secs: u64,
    ) -> Result<Vec<TorrentFileInfo>, String> {
        let session = self.get_session()?;

        if let Ok(magnet) = bt::Magnet::parse(magnet_uri) {
            if let Some(handle) = session.get(bt::TorrentIdOrHash::Hash(magnet.info_hash())) {
                if let Ok(files) = handle.with_metadata(|meta| extract_file_details(&meta.info)) {
                    log::info!(
                        "Magnet already managed, resolved from session ({} files)",
                        files.len()
                    );
                    return Ok(files);
                }
            }
        }

        let trackers = Self::parse_trackers(options);
        log::info!("Resolving magnet metadata: {}", magnet_uri);
        let start = std::time::Instant::now();

        let resolved = tokio::time::timeout(
            Duration::from_secs(timeout_secs),
            bt::magnet::resolve(magnet_uri, &trackers, Duration::from_secs(timeout_secs)),
        )
        .await
        .map_err(|_| "Timed out resolving magnet metadata".to_string())?
        .map_err(|e| format!("Failed to resolve magnet: {}", e))?;

        let torrent_bytes =
            bt::magnet::synth_torrent_bytes(&resolved.info_bytes, &resolved.trackers);
        let meta = bt::parse_torrent(&torrent_bytes)
            .map_err(|e| format!("Failed to parse resolved metadata: {}", e))?;
        let files = extract_file_details(&meta.info);
        log::info!(
            "Magnet metadata resolved in {:?} ({} files)",
            start.elapsed(),
            files.len()
        );
        Ok(files)
    }

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

    pub fn get_torrent_stats(&self, torrent_id: usize) -> Option<TorrentStats> {
        let session = self.session.as_ref()?;
        let handle = session.get(bt::TorrentIdOrHash::Id(torrent_id))?;
        let stats = handle.stats();

        let (download_speed, upload_speed, num_peers) = match &stats.live {
            Some(live) => {
                let peers = live.snapshot.peer_stats.live;
                let dl = (live.download_speed.mbps * 1_048_576.0) as u64;
                let ul = (live.upload_speed.mbps * 1_048_576.0) as u64;
                (dl, ul, peers)
            }
            None => (0, 0, 0),
        };

        let name = handle.name();

        let file_details = handle
            .metadata
            .load()
            .as_ref()
            .map(|meta| extract_file_details(&meta.info));

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

    pub async fn pause(&self, torrent_id: usize) -> Result<(), String> {
        let session = self.get_session()?;
        let handle = session
            .get(bt::TorrentIdOrHash::Id(torrent_id))
            .ok_or("Torrent not found")?;
        session
            .pause(&handle)
            .await
            .map_err(|e| format!("Failed to pause: {}", e))
    }

    pub async fn unpause(&self, torrent_id: usize) -> Result<(), String> {
        let session = self.get_session()?;
        let handle = session
            .get(bt::TorrentIdOrHash::Id(torrent_id))
            .ok_or("Torrent not found")?;
        session
            .unpause(&handle)
            .await
            .map_err(|e| format!("Failed to unpause: {}", e))
    }

    pub async fn remove(&self, torrent_id: usize) -> Result<(), String> {
        let session = self.get_session()?;
        session
            .delete(bt::TorrentIdOrHash::Id(torrent_id), false)
            .await
            .map_err(|e| format!("Failed to remove torrent: {}", e))
    }

    pub async fn shutdown(&mut self) {
        if let Some(session) = self.session.take() {
            drop(session);
        }
    }
}

fn extract_handle(response: bt::AddTorrentResponse) -> Result<TorrentHandle, String> {
    match response {
        bt::AddTorrentResponse::Added(id, handle)
        | bt::AddTorrentResponse::AlreadyManaged(id, handle) => Ok(TorrentHandle {
            id,
            info_hash: Some(handle.info_hash().as_string()),
        }),
        bt::AddTorrentResponse::ListOnly(_) => {
            Err("Torrent was added in list-only mode".to_string())
        }
    }
}

fn extract_file_details(info: &bt::ValidatedTorrentMetaV1Info) -> Vec<TorrentFileInfo> {
    info.iter_file_details()
        .enumerate()
        .map(|(idx, d)| TorrentFileInfo {
            index: idx,
            path: d.filename.to_string(),
            length: d.len,
        })
        .collect()
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

pub fn is_magnet_uri(uri: &str) -> bool {
    uri.trim().to_lowercase().starts_with("magnet:")
}

/// Strip filesystem-unsafe characters from a torrent name so it can be used
/// as a filename stem on all platforms. Returns a non-empty placeholder for
/// names that reduce to whitespace.
fn sanitize_file_stem(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').to_string();
    if trimmed.is_empty() {
        "torrent".to_string()
    } else {
        trimmed
    }
}
