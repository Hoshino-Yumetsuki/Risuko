use risuko_bt as bt;
use serde_json::{Map, Value};
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// BitTorrent session tuning passed from the user/system config. All
/// fields are optional; missing entries fall back to `risuko-bt` defaults
#[derive(Debug, Default, Clone)]
pub struct BtTuning {
    pub max_outstanding_per_peer: Option<usize>,
    pub max_peers_per_torrent: Option<usize>,
    pub enable_upnp: Option<bool>,
    pub upnp_lease: Option<Duration>,
    /// Accepts "plaintext", "prefer", or "require". Anything else is ignored
    pub encryption_policy: Option<String>,
    pub listen_ipv6: Option<bool>,
    pub enable_lsd: Option<bool>,
}

/// Read-only BT session diagnostics used by the `/health` panel
#[derive(Debug, Clone, Copy)]
pub struct BtHealthSnapshot {
    pub listen_port: u16,
    pub lsd_active: bool,
    pub upnp_enabled: bool,
    pub upnp_mappings: usize,
    pub upnp_attempts: usize,
    pub torrents: usize,
    pub dht_active: bool,
    pub dht_nodes: usize,
}

/// BitTorrent download management via the in-tree `risuko-bt` engine
pub struct TorrentEngine {
    session: Option<Arc<bt::Session>>,
    output_dir: PathBuf,
}

impl TorrentEngine {
    pub async fn new(output_dir: &Path) -> Result<Self, String> {
        Self::new_with_tuning(output_dir, BtTuning::default()).await
    }

    pub async fn new_with_tuning(output_dir: &Path, tuning: BtTuning) -> Result<Self, String> {
        std::fs::create_dir_all(output_dir)
            .map_err(|e| format!("Failed to create torrent output dir: {}", e))?;

        let encryption = encryption_policy_from_str(tuning.encryption_policy.as_deref());

        let session = bt::Session::new_with_opts(
            output_dir.to_path_buf(),
            bt::SessionOptions {
                listen: Some(bt::ListenerOptions {
                    listen_addr: Some((Ipv4Addr::UNSPECIFIED, 0).into()),
                    enable_upnp_port_forwarding: tuning.enable_upnp.unwrap_or(true),
                    upnp_lease: tuning.upnp_lease,
                    listen_ipv6: tuning.listen_ipv6.unwrap_or(false),
                }),
                fastresume: true,
                // Persist session state so managed torrents survive restarts;
                // without this, `fastresume` has nothing to resume from.
                persistence: Some(bt::SessionPersistenceConfig::Json {
                    folder: Some(output_dir.to_path_buf()),
                }),
                max_outstanding_requests_per_peer: tuning.max_outstanding_per_peer,
                max_peers_per_torrent: tuning.max_peers_per_torrent,
                disable_local_service_discovery: !tuning.enable_lsd.unwrap_or(true),
                encryption,
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

    /// Snapshot of BitTorrent session health for the `/health` panel
    /// Returns `None` when the torrent engine has been torn down
    pub fn health_snapshot(&self) -> Option<BtHealthSnapshot> {
        let session = self.session.as_ref()?;
        let upnp = session.upnp_status();
        Some(BtHealthSnapshot {
            listen_port: session.listen_port(),
            lsd_active: session.lsd_active(),
            upnp_enabled: upnp.enabled,
            upnp_mappings: upnp.mapping_count,
            upnp_attempts: upnp.discovery_attempts,
            torrents: session.with_torrents(|i| i.count()),
            dht_active: session.dht_active(),
            dht_nodes: session.dht_routing_table_len(),
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
        let create_subfolder = options
            .get("bt-create-subfolder")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

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
            create_subfolder,
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
        let create_subfolder = options
            .get("bt-create-subfolder")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

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
            create_subfolder,
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
            let enc = encryption_policy_from_str(
                options.get("bt-encryption-policy").and_then(|v| v.as_str()),
            );
            match bt::magnet::resolve(magnet_uri, &trackers, Duration::from_secs(120), enc).await {
                Ok(resolved) => {
                    let bytes = bt::magnet::synth_torrent_bytes(
                        &resolved.info_bytes,
                        &resolved.trackers,
                        &resolved.piece_layers,
                    );
                    if let Ok(meta) = bt::parse_torrent(&bytes) {
                        let name = if meta.info.name.is_empty() {
                            format!("{:?}", meta.info_hash)
                        } else {
                            meta.info.name.clone()
                        };
                        let safe = sanitize_file_stem(&name);
                        let path = Path::new(dir).join(format!("{}.torrent", safe));
                        // Ensure custom `dir` values exist before writing the
                        // synthesized .torrent file.
                        if let Some(parent) = path.parent() {
                            if let Err(e) = tokio::fs::create_dir_all(parent).await {
                                log::warn!(
                                    "Failed to create metadata dir {}: {}",
                                    parent.display(),
                                    e
                                );
                            }
                        }
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

        let enc = encryption_policy_from_str(
            options.get("bt-encryption-policy").and_then(|v| v.as_str()),
        );
        let resolved = tokio::time::timeout(
            Duration::from_secs(timeout_secs),
            bt::magnet::resolve(
                magnet_uri,
                &trackers,
                Duration::from_secs(timeout_secs),
                enc,
            ),
        )
        .await
        .map_err(|_| "Timed out resolving magnet metadata".to_string())?
        .map_err(|e| format!("Failed to resolve magnet: {}", e))?;

        let torrent_bytes = bt::magnet::synth_torrent_bytes(
            &resolved.info_bytes,
            &resolved.trackers,
            &resolved.piece_layers,
        );
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

        let (download_speed, upload_speed, num_peers, peers, num_seeders) = match &stats.live {
            Some(live) => {
                let count = live.snapshot.peer_stats.live;
                let dl = (live.download_speed.mbps * 1_048_576.0) as u64;
                let ul = (live.upload_speed.mbps * 1_048_576.0) as u64;
                let mapped: Vec<PeerSnapshot> = stats
                    .peers
                    .iter()
                    .map(|p| PeerSnapshot {
                        addr: p.addr,
                        bitfield: p.bitfield.clone(),
                        am_choking: p.am_choking,
                        am_interested: p.am_interested,
                        peer_choking: p.peer_choking,
                        peer_interested: p.peer_interested,
                        seeder: p.seeder,
                    })
                    .collect();
                let seeders = mapped.iter().filter(|p| p.seeder).count() as u32;
                (dl, ul, count, mapped, seeders)
            }
            None => (0, 0, 0, Vec::new(), 0),
        };

        let name = handle.name();

        let metadata_payload = handle.metadata.load();
        let metadata = metadata_payload.as_ref().map(|meta| {
            let total_pieces = (meta.info.pieces.len() / 20) as u32;
            TorrentMetadataInfo {
                piece_length: meta.info.piece_length,
                num_pieces: total_pieces,
                comment: meta.comment.clone(),
                creation_date: meta.creation_date,
                announce_list: build_announce_list(meta),
            }
        });
        let file_details = metadata_payload
            .as_ref()
            .map(|meta| extract_file_details(&meta.info));

        Some(TorrentStats {
            total_bytes: stats.total_bytes,
            downloaded_bytes: stats.progress_bytes,
            uploaded_bytes: stats.uploaded_bytes,
            download_speed,
            upload_speed,
            num_peers,
            num_seeders,
            is_finished: stats.finished,
            name,
            file_progress: stats.file_progress,
            file_details,
            peers,
            metadata,
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

    /// Drop a torrent from the bt session
    ///
    /// `with_files = true` also wipes the on-disk payload (used by
    /// "remove task" / orphan purge). `with_files = false` keeps files but
    /// still releases the in-memory `by_hash` reservation, which is required
    /// by `remove_download_result` / `purge_download_result` so re-adding
    /// the same magnet does not short-circuit to `AlreadyManaged` and
    /// surface as "magnet shows complete with no download" until restart
    pub async fn remove(&self, torrent_id: usize, with_files: bool) -> Result<(), String> {
        let session = self.get_session()?;
        session
            .delete(bt::TorrentIdOrHash::Id(torrent_id), with_files)
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
            info_hash_v2: handle.info_hash_v2().map(|h| h.as_string()),
            meta_version: handle.meta_version().map(|s| s.to_string()),
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

/// Aggregate `announce` and `announce_list` into the BEP-12 nested-tier
/// shape expected by the frontend (`string[][]`). Falls back to a single
/// tier containing the primary announce URL when no list is present
fn build_announce_list(meta: &bt::TorrentMeta) -> Vec<Vec<String>> {
    if !meta.announce_list.is_empty() {
        return meta
            .announce_list
            .iter()
            .map(|tier| tier.to_vec())
            .filter(|tier: &Vec<String>| !tier.is_empty())
            .collect();
    }
    match &meta.announce {
        Some(url) if !url.is_empty() => vec![vec![url.clone()]],
        _ => Vec::new(),
    }
}

pub struct TorrentHandle {
    pub id: usize,
    pub info_hash: Option<String>,
    pub info_hash_v2: Option<String>,
    pub meta_version: Option<String>,
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
    pub num_seeders: u32,
    pub is_finished: bool,
    pub name: Option<String>,
    pub file_progress: Vec<u64>,
    pub file_details: Option<Vec<TorrentFileInfo>>,
    pub peers: Vec<PeerSnapshot>,
    pub metadata: Option<TorrentMetadataInfo>,
}

pub struct PeerSnapshot {
    pub addr: std::net::SocketAddr,
    /// Raw bitfield bytes; manager hex-encodes for the RPC payload
    pub bitfield: Vec<u8>,
    pub am_choking: bool,
    pub am_interested: bool,
    pub peer_choking: bool,
    pub peer_interested: bool,
    pub seeder: bool,
}

pub struct TorrentMetadataInfo {
    pub piece_length: u32,
    pub num_pieces: u32,
    pub comment: Option<String>,
    pub creation_date: Option<i64>,
    pub announce_list: Vec<Vec<String>>,
}

pub fn is_magnet_uri(uri: &str) -> bool {
    uri.trim().to_lowercase().starts_with("magnet:")
}

/// Strip filesystem-unsafe characters from a torrent name so it can be used
/// as a filename stem on all platforms. Returns a non-empty placeholder for
/// names that reduce to whitespace
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

/// Map an optional config string to a concrete BitTorrent encryption
/// policy. Unknown / missing values fall back to `Prefer` (MSE first,
/// plaintext fallback) which matches the system default
fn encryption_policy_from_str(s: Option<&str>) -> bt::EncryptionPolicy {
    match s {
        Some("plaintext") => bt::EncryptionPolicy::PlaintextOnly,
        Some("require") => bt::EncryptionPolicy::RequireEncryption,
        _ => bt::EncryptionPolicy::Prefer,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encryption_policy_known_values() {
        assert!(matches!(
            encryption_policy_from_str(Some("plaintext")),
            bt::EncryptionPolicy::PlaintextOnly
        ));
        assert!(matches!(
            encryption_policy_from_str(Some("prefer")),
            bt::EncryptionPolicy::Prefer
        ));
        assert!(matches!(
            encryption_policy_from_str(Some("require")),
            bt::EncryptionPolicy::RequireEncryption
        ));
    }

    #[test]
    fn encryption_policy_unknown_falls_back_to_prefer() {
        assert!(matches!(
            encryption_policy_from_str(None),
            bt::EncryptionPolicy::Prefer
        ));
        assert!(matches!(
            encryption_policy_from_str(Some("garbage")),
            bt::EncryptionPolicy::Prefer
        ));
        // case-sensitive: uppercase is not recognised
        assert!(matches!(
            encryption_policy_from_str(Some("REQUIRE")),
            bt::EncryptionPolicy::Prefer
        ));
    }

    #[test]
    fn sanitize_file_stem_replaces_separators() {
        assert_eq!(sanitize_file_stem("a/b\\c"), "a_b_c");
        assert_eq!(sanitize_file_stem("  ..hidden..  "), "hidden");
        assert_eq!(sanitize_file_stem(""), "torrent");
        // Control chars become '_'; result is non-empty so we keep it.
        assert_eq!(sanitize_file_stem("\0\n\t"), "___");
    }
}
