use serde_json::{Map, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use super::events::{EngineEvent, EventBroadcaster};
use super::http;
use super::options::EngineOptions;
use super::session::SessionManager;
use super::speed_limiter::{parse_speed_limit, SpeedLimiter};
use super::task::{generate_gid, DownloadFile, DownloadTask, FileUri, TaskKind, TaskStatus};
use super::torrent::TorrentEngine;

struct ActiveDownload {
    cancel: Arc<AtomicBool>,
    cancel_token: CancellationToken,
    total: Arc<AtomicU64>,
    completed: Arc<AtomicU64>,
    speed: Arc<AtomicU64>,
    connections: Arc<AtomicU32>,
}

pub struct TaskManager {
    tasks: Arc<RwLock<Vec<DownloadTask>>>,
    active_downloads: Arc<RwLock<HashMap<String, ActiveDownload>>>,
    torrent_ids: Arc<RwLock<HashMap<String, usize>>>,
    options: Arc<RwLock<EngineOptions>>,
    events: EventBroadcaster,
    session: SessionManager,
    torrent_engine: Arc<RwLock<Option<TorrentEngine>>>,
    global_speed_limiter: Arc<SpeedLimiter>,
}

impl TaskManager {
    pub async fn new(
        config_dir: &Path,
        options: EngineOptions,
        events: EventBroadcaster,
    ) -> Result<Self, String> {
        let session = SessionManager::new(config_dir);
        let saved_tasks = session.load();

        let output_dir = options.dir();
        let torrent_engine = TorrentEngine::new(Path::new(&output_dir))
            .await
            .map_err(|e| {
                log::warn!("Torrent engine init failed (non-fatal): {}", e);
                e
            })
            .ok();

        let global_speed_limiter = Arc::new(SpeedLimiter::new(
            options.max_overall_download_limit(),
        ));

        let manager = Self {
            tasks: Arc::new(RwLock::new(saved_tasks)),
            active_downloads: Arc::new(RwLock::new(HashMap::new())),
            torrent_ids: Arc::new(RwLock::new(HashMap::new())),
            options: Arc::new(RwLock::new(options)),
            events,
            session,
            torrent_engine: Arc::new(RwLock::new(torrent_engine)),
            global_speed_limiter,
        };

        // Restore torrent_ids mapping from persisted librqbit session
        manager.restore_torrent_mappings().await;

        Ok(manager)
    }

    /// restarted, match persisted librqbit torrents back to saved tasks by info_hash
    async fn restore_torrent_mappings(&self) {
        let te_guard = self.torrent_engine.read().await;
        let Some(ref te) = *te_guard else { return };

        let managed = te.list_managed_torrents();
        if managed.is_empty() {
            return;
        }

        let mut tasks = self.tasks.write().await;
        let mut ids = self.torrent_ids.write().await;

        for (librqbit_id, info_hash) in &managed {
            for task in tasks.iter_mut() {
                if task.kind == TaskKind::Torrent
                    && task.info_hash.as_deref() == Some(info_hash.as_str())
                    && task.status != TaskStatus::Removed
                {
                    ids.insert(task.gid.clone(), *librqbit_id);
                    // Session load sets active to paused, but librqbit is still running
                    // Restore active so update_progress can track it
                    if task.status == TaskStatus::Paused {
                        task.status = TaskStatus::Active;
                    }
                    log::info!(
                        "Restored torrent mapping: gid={} -> librqbit_id={} ({})",
                        task.gid, librqbit_id, info_hash
                    );
                    break;
                }
            }
        }

        log::info!("Restored {} torrent mappings out of {} persisted torrents",
            ids.len(), managed.len());
    }

    pub async fn add_http_task(
        &self,
        uris: Vec<String>,
        options: Map<String, Value>,
    ) -> Result<String, String> {
        let gid = generate_gid();
        let dir = {
            let merged = self.options.read().await.merge_task_options(&options);
            merged
                .get("dir")
                .and_then(|v| v.as_str())
                .unwrap_or(".")
                .to_string()
        };

        // Only honor pause if explicitly set in per-task options (not from global defaults)
        let pause = options
            .get("pause")
            .and_then(|v| v.as_bool().or_else(|| v.as_str().map(|s| s == "true")))
            .unwrap_or(false);

        let task = DownloadTask::new_http(gid.clone(), uris, dir, options);
        self.tasks.write().await.push(task);

        if !pause {
            self.try_start_next().await;
        }

        self.events
            .send(EngineEvent::DownloadStart { gid: gid.clone() });

        Ok(gid)
    }

    pub async fn add_torrent_task(
        &self,
        torrent_data: Vec<u8>,
        options: Map<String, Value>,
    ) -> Result<String, String> {
        let gid = generate_gid();
        let merged = self.options.read().await.merge_task_options(&options);
        let dir = merged
            .get("dir")
            .and_then(|v| v.as_str())
            .unwrap_or(".")
            .to_string();

        let mut task = DownloadTask::new_torrent(gid.clone(), dir.clone(), options.clone());

        // Add to torrent engine
        let te_guard = self.torrent_engine.read().await;
        if let Some(ref te) = *te_guard {
            match te.add_torrent_bytes(&torrent_data, &merged).await {
                Ok(handle) => {
                    log::info!("Torrent task {} added: id={}, info_hash={:?}", gid, handle.id, handle.info_hash);
                    self.torrent_ids.write().await.insert(gid.clone(), handle.id);
                    task.info_hash = handle.info_hash;
                    task.status = TaskStatus::Active;
                }
                Err(e) => {
                    log::error!("Torrent task {} failed to add: {}", gid, e);
                    task.status = TaskStatus::Error;
                    task.error_message = Some(e);
                }
            }
        } else {
            log::error!("Torrent task {} failed: engine not available", gid);
            task.status = TaskStatus::Error;
            task.error_message = Some("Torrent engine not available".to_string());
        }
        drop(te_guard);

        self.tasks.write().await.push(task);
        self.events
            .send(EngineEvent::DownloadStart { gid: gid.clone() });

        Ok(gid)
    }

    pub async fn add_magnet_task(
        &self,
        magnet_uri: &str,
        options: Map<String, Value>,
    ) -> Result<String, String> {
        let gid = generate_gid();
        let merged = self.options.read().await.merge_task_options(&options);
        let dir = merged
            .get("dir")
            .and_then(|v| v.as_str())
            .unwrap_or(".")
            .to_string();

        let mut task = DownloadTask::new_torrent(gid.clone(), dir.clone(), options.clone());
        task.uris = vec![magnet_uri.to_string()];

        let te_guard = self.torrent_engine.read().await;
        if let Some(ref te) = *te_guard {
            match te.add_magnet(magnet_uri, &merged).await {
                Ok(handle) => {
                    self.torrent_ids.write().await.insert(gid.clone(), handle.id);
                    task.info_hash = handle.info_hash;
                    task.status = TaskStatus::Active;
                }
                Err(e) => {
                    task.status = TaskStatus::Error;
                    task.error_message = Some(e);
                }
            }
        } else {
            task.status = TaskStatus::Error;
            task.error_message = Some("Torrent engine not available".to_string());
        }
        drop(te_guard);

        self.tasks.write().await.push(task);
        self.events
            .send(EngineEvent::DownloadStart { gid: gid.clone() });

        Ok(gid)
    }

    pub async fn add_ed2k_task(
        &self,
        uri: &str,
        options: Map<String, Value>,
    ) -> Result<String, String> {
        let link = super::ed2k::parse_ed2k_link(uri)?;
        let gid = generate_gid();
        let merged = self.options.read().await.merge_task_options(&options);
        let dir = merged
            .get("dir")
            .and_then(|v| v.as_str())
            .unwrap_or(".")
            .to_string();

        let file_name = link.file_name.clone();
        let file_size = link.file_size;

        // Only honor pause if explicitly set in per-task options
        let pause = options
            .get("pause")
            .and_then(|v| v.as_bool().or_else(|| v.as_str().map(|s| s == "true")))
            .unwrap_or(false);

        let task = DownloadTask::new_ed2k(
            gid.clone(),
            uri.to_string(),
            file_name,
            file_size,
            dir,
            options,
        );

        self.tasks.write().await.push(task);

        if !pause {
            self.try_start_next().await;
        }

        self.events
            .send(EngineEvent::DownloadStart { gid: gid.clone() });

        Ok(gid)
    }

    pub async fn add_m3u8_task(
        &self,
        uri: &str,
        options: Map<String, Value>,
    ) -> Result<String, String> {
        let gid = generate_gid();
        let merged = self.options.read().await.merge_task_options(&options);
        let dir = merged
            .get("dir")
            .and_then(|v| v.as_str())
            .unwrap_or(".")
            .to_string();

        // Infer output filename: strip .m3u8 → .ts
        let out = options
            .get("out")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| infer_m3u8_output_name(uri));

        let pause = options
            .get("pause")
            .and_then(|v| v.as_bool().or_else(|| v.as_str().map(|s| s == "true")))
            .unwrap_or(false);

        let task = DownloadTask::new_m3u8(gid.clone(), uri.to_string(), out, dir, options);
        self.tasks.write().await.push(task);

        if !pause {
            self.try_start_next().await;
        }

        self.events
            .send(EngineEvent::DownloadStart { gid: gid.clone() });

        Ok(gid)
    }

    pub async fn add_ftp_task(
        &self,
        uri: &str,
        options: Map<String, Value>,
    ) -> Result<String, String> {
        let gid = generate_gid();
        let merged = self.options.read().await.merge_task_options(&options);
        let dir = merged
            .get("dir")
            .and_then(|v| v.as_str())
            .unwrap_or(".")
            .to_string();

        let pause = options
            .get("pause")
            .and_then(|v| v.as_bool().or_else(|| v.as_str().map(|s| s == "true")))
            .unwrap_or(false);

        let task = DownloadTask::new_ftp(gid.clone(), uri.to_string(), dir, options);
        self.tasks.write().await.push(task);

        if !pause {
            self.try_start_next().await;
        }

        self.events
            .send(EngineEvent::DownloadStart { gid: gid.clone() });

        Ok(gid)
    }

    /// Start download workers for waiting tasks up to max concurrent limit
    async fn try_start_next(&self) {
        let options_guard = self.options.read().await;
        let max_concurrent = options_guard.max_concurrent_downloads();
        let active_count = self.active_downloads.read().await.len();

        if active_count >= max_concurrent {
            return;
        }

        let slots = max_concurrent - active_count;
        let mut tasks = self.tasks.write().await;
        let mut started = 0;

        for task in tasks.iter_mut() {
            if started >= slots {
                break;
            }
            if task.status != TaskStatus::Waiting {
                continue;
            }
            if task.kind == TaskKind::Http && !task.uris.is_empty() {
                task.status = TaskStatus::Active;
                let merged = options_guard.merge_task_options(&task.options);
                self.spawn_http_download(task, merged);
                started += 1;
            } else if task.kind == TaskKind::M3u8 && !task.uris.is_empty() {
                task.status = TaskStatus::Active;
                let merged = options_guard.merge_task_options(&task.options);
                self.spawn_m3u8_download(task, merged);
                started += 1;
            } else if task.kind == TaskKind::Ed2k && !task.uris.is_empty() {
                task.status = TaskStatus::Active;
                self.spawn_ed2k_download(task);
                started += 1;
            } else if task.kind == TaskKind::Ftp && !task.uris.is_empty() {
                task.status = TaskStatus::Active;
                let merged = options_guard.merge_task_options(&task.options);
                self.spawn_ftp_download(task, merged);
                started += 1;
            }
        }
    }

    fn spawn_http_download(&self, task: &DownloadTask, merged_options: Map<String, Value>) {
        let gid = task.gid.clone();
        let uri = task.uris.first().cloned().unwrap_or_default();
        let dir = task.dir.clone();
        let out = task.out.clone();
        let events = self.events.clone();
        let tasks = self.tasks.clone();
        let active = self.active_downloads.clone();

        let split: u32 = merged_options
            .get("split")
            .and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
            .unwrap_or(1)
            .max(1) as u32;

        // Per-task speed limit from merged options
        let per_task_limit = merged_options
            .get("max-download-limit")
            .map(parse_speed_limit)
            .unwrap_or(0);
        let task_speed_limiter = Arc::new(SpeedLimiter::new(per_task_limit));
        let global_limiter = self.global_speed_limiter.clone();

        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_token = CancellationToken::new();
        let total = Arc::new(AtomicU64::new(0));
        let completed = Arc::new(AtomicU64::new(0));
        let speed = Arc::new(AtomicU64::new(0));
        let connections = Arc::new(AtomicU32::new(split));

        let cancel_dl = cancel.clone();
        let cancel_token_dl = cancel_token.clone();
        let total_dl = total.clone();
        let completed_dl = completed.clone();
        let speed_dl = speed.clone();
        let connections_dl = connections.clone();

        let total_ref = total.clone();
        let completed_ref = completed.clone();

        let gid_clone = gid.clone();

        let active_for_insert = active.clone();
        let gid_for_insert = gid.clone();
        let completion_handle = tokio::spawn(async move {
            let ad = ActiveDownload {
                cancel,
                cancel_token: cancel_token.clone(),
                total,
                completed,
                speed,
                connections,
            };
            active_for_insert.write().await.insert(gid_for_insert, ad);

            let download_result = http::run_http_download(
                &uri,
                &dir,
                &out,
                &merged_options,
                total_dl,
                completed_dl,
                speed_dl,
                cancel_dl,
                connections_dl,
                cancel_token_dl,
                global_limiter,
                task_speed_limiter,
            )
            .await;

            // Update task status
            let mut tasks_guard = tasks.write().await;
            if let Some(task) = tasks_guard.iter_mut().find(|t| t.gid == gid_clone) {
                task.total_length = total_ref.load(Ordering::Relaxed);
                task.completed_length = completed_ref.load(Ordering::Relaxed);
                task.download_speed = 0;

                match download_result {
                    Ok(path) => {
                        task.status = TaskStatus::Complete;
                        task.files = vec![DownloadFile {
                            index: "1".to_string(),
                            path: path.to_string_lossy().to_string(),
                            length: task.total_length.to_string(),
                            completed_length: task.total_length.to_string(),
                            selected: "true".to_string(),
                            uris: task
                                .uris
                                .iter()
                                .map(|u| FileUri {
                                    uri: u.clone(),
                                    status: "used".to_string(),
                                })
                                .collect(),
                        }];
                        events.send(EngineEvent::DownloadComplete {
                            gid: gid_clone.clone(),
                        });
                    }
                    Err(e) => {
                        if e.contains("cancelled") {
                            // Don't overwrite Removed status from remove()
                            if task.status != TaskStatus::Removed {
                                task.status = TaskStatus::Paused;
                            }
                            events.send(EngineEvent::DownloadPause {
                                gid: gid_clone.clone(),
                            });
                        } else {
                            tracing::error!("Download failed for {}: {}", gid_clone, e);
                            task.status = TaskStatus::Error;
                            task.error_code = Some("1".to_string());
                            task.error_message = Some(e);
                            events.send(EngineEvent::DownloadError {
                                gid: gid_clone.clone(),
                            });
                        }
                    }
                }
            }
            drop(tasks_guard);

            // Remove from active downloads
            active.write().await.remove(&gid_clone);
        });

        // Detach, the completion_handle runs independently
        drop(completion_handle);
    }

    fn spawn_ed2k_download(&self, task: &DownloadTask) {
        let gid = task.gid.clone();
        let uri = task.uris.first().cloned().unwrap_or_default();
        let dir = task.dir.clone();
        let events = self.events.clone();
        let tasks = self.tasks.clone();
        let active = self.active_downloads.clone();
        let options = self.options.clone();

        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_token = CancellationToken::new();
        let total = Arc::new(AtomicU64::new(task.total_length));
        let completed = Arc::new(AtomicU64::new(0));
        let speed = Arc::new(AtomicU64::new(0));
        let connections = Arc::new(AtomicU32::new(0));

        let cancel_dl = cancel.clone();
        let cancel_token_dl = cancel_token.clone();
        let total_dl = total.clone();
        let completed_dl = completed.clone();
        let speed_dl = speed.clone();
        let connections_dl = connections.clone();

        let total_ref = total.clone();
        let completed_ref = completed.clone();
        let gid_clone = gid.clone();

        let active_for_insert = active.clone();
        let gid_for_insert = gid.clone();
        let completion_handle = tokio::spawn(async move {
            let ad = ActiveDownload {
                cancel,
                cancel_token: cancel_token.clone(),
                total,
                completed,
                speed,
                connections,
            };
            active_for_insert.write().await.insert(gid_for_insert, ad);

            let file_link = super::ed2k::parse_ed2k_link(&uri);
            let opts_guard = options.read().await;
            let ed2k_servers = opts_guard.ed2k_servers();
            let ed2k_port = opts_guard.ed2k_port();
            drop(opts_guard);

            let download_result = match file_link {
                Ok(link) => {
                    super::ed2k::run_ed2k_download(
                        &link,
                        &dir,
                        ed2k_servers,
                        ed2k_port,
                        total_dl,
                        completed_dl,
                        speed_dl,
                        cancel_dl,
                        connections_dl,
                        cancel_token_dl,
                    )
                    .await
                }
                Err(e) => Err(e),
            };

            // Update task status
            let mut tasks_guard = tasks.write().await;
            if let Some(task) = tasks_guard.iter_mut().find(|t| t.gid == gid_clone) {
                task.total_length = total_ref.load(Ordering::Relaxed);
                task.completed_length = completed_ref.load(Ordering::Relaxed);
                task.download_speed = 0;

                match download_result {
                    Ok(path) => {
                        task.status = TaskStatus::Complete;
                        task.files = vec![DownloadFile {
                            index: "1".to_string(),
                            path: path.to_string_lossy().to_string(),
                            length: task.total_length.to_string(),
                            completed_length: task.total_length.to_string(),
                            selected: "true".to_string(),
                            uris: task
                                .uris
                                .iter()
                                .map(|u| FileUri {
                                    uri: u.clone(),
                                    status: "used".to_string(),
                                })
                                .collect(),
                        }];
                        events.send(EngineEvent::DownloadComplete {
                            gid: gid_clone.clone(),
                        });
                    }
                    Err(e) => {
                        if e.contains("cancelled") {
                            // Don't overwrite Removed status from remove()
                            if task.status != TaskStatus::Removed {
                                task.status = TaskStatus::Paused;
                            }
                            events.send(EngineEvent::DownloadPause {
                                gid: gid_clone.clone(),
                            });
                        } else {
                            tracing::error!("[ed2k] Download failed for {}: {}", gid_clone, e);
                            task.status = TaskStatus::Error;
                            task.error_code = Some("1".to_string());
                            task.error_message = Some(e);
                            events.send(EngineEvent::DownloadError {
                                gid: gid_clone.clone(),
                            });
                        }
                    }
                }
            }
            drop(tasks_guard);

            // Remove from active downloads
            active.write().await.remove(&gid_clone);
        });

        drop(completion_handle);
    }

    fn spawn_m3u8_download(&self, task: &DownloadTask, merged_options: Map<String, Value>) {
        let gid = task.gid.clone();
        let uri = task.uris.first().cloned().unwrap_or_default();
        let dir = task.dir.clone();
        let out = task.out.clone();
        let events = self.events.clone();
        let tasks = self.tasks.clone();
        let active = self.active_downloads.clone();

        let per_task_limit = merged_options
            .get("max-download-limit")
            .map(parse_speed_limit)
            .unwrap_or(0);
        let task_speed_limiter = Arc::new(SpeedLimiter::new(per_task_limit));
        let global_limiter = self.global_speed_limiter.clone();

        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_token = CancellationToken::new();
        let total = Arc::new(AtomicU64::new(0));
        let completed = Arc::new(AtomicU64::new(0));
        let speed = Arc::new(AtomicU64::new(0));
        let connections = Arc::new(AtomicU32::new(0));

        let cancel_dl = cancel.clone();
        let cancel_token_dl = cancel_token.clone();
        let total_dl = total.clone();
        let completed_dl = completed.clone();
        let speed_dl = speed.clone();
        let connections_dl = connections.clone();

        let total_ref = total.clone();
        let completed_ref = completed.clone();
        let gid_clone = gid.clone();

        let active_for_insert = active.clone();
        let gid_for_insert = gid.clone();
        let completion_handle = tokio::spawn(async move {
            let ad = ActiveDownload {
                cancel,
                cancel_token: cancel_token.clone(),
                total,
                completed,
                speed,
                connections,
            };
            active_for_insert.write().await.insert(gid_for_insert, ad);

            let download_result = super::m3u8::run_m3u8_download(
                &uri,
                &dir,
                &out,
                &merged_options,
                total_dl,
                completed_dl,
                speed_dl,
                cancel_dl,
                connections_dl,
                cancel_token_dl,
                global_limiter,
                task_speed_limiter,
            )
            .await;

            let mut tasks_guard = tasks.write().await;
            if let Some(task) = tasks_guard.iter_mut().find(|t| t.gid == gid_clone) {
                task.total_length = total_ref.load(Ordering::Relaxed);
                task.completed_length = completed_ref.load(Ordering::Relaxed);
                task.download_speed = 0;

                match download_result {
                    Ok(path) => {
                        task.status = TaskStatus::Complete;
                        task.files = vec![DownloadFile {
                            index: "1".to_string(),
                            path: path.to_string_lossy().to_string(),
                            length: task.total_length.to_string(),
                            completed_length: task.total_length.to_string(),
                            selected: "true".to_string(),
                            uris: task
                                .uris
                                .iter()
                                .map(|u| FileUri {
                                    uri: u.clone(),
                                    status: "used".to_string(),
                                })
                                .collect(),
                        }];
                        events.send(EngineEvent::DownloadComplete {
                            gid: gid_clone.clone(),
                        });
                    }
                    Err(e) => {
                        if e.contains("cancelled") {
                            // Don't overwrite Removed status from remove()
                            if task.status != TaskStatus::Removed {
                                task.status = TaskStatus::Paused;
                            }
                            events.send(EngineEvent::DownloadPause {
                                gid: gid_clone.clone(),
                            });
                        } else {
                            tracing::error!("[m3u8] Download failed for {}: {}", gid_clone, e);
                            task.status = TaskStatus::Error;
                            task.error_code = Some("1".to_string());
                            task.error_message = Some(e);
                            events.send(EngineEvent::DownloadError {
                                gid: gid_clone.clone(),
                            });
                        }
                    }
                }
            }
            drop(tasks_guard);

            active.write().await.remove(&gid_clone);
        });

        drop(completion_handle);
    }

    fn spawn_ftp_download(&self, task: &DownloadTask, merged_options: Map<String, Value>) {
        let gid = task.gid.clone();
        let uri = task.uris.first().cloned().unwrap_or_default();
        let dir = task.dir.clone();
        let out = task.out.clone();
        let events = self.events.clone();
        let tasks = self.tasks.clone();
        let active = self.active_downloads.clone();

        let per_task_limit = merged_options
            .get("max-download-limit")
            .map(parse_speed_limit)
            .unwrap_or(0);
        let task_speed_limiter = Arc::new(SpeedLimiter::new(per_task_limit));
        let global_limiter = self.global_speed_limiter.clone();

        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_token = CancellationToken::new();
        let total = Arc::new(AtomicU64::new(0));
        let completed = Arc::new(AtomicU64::new(0));
        let speed = Arc::new(AtomicU64::new(0));
        let connections = Arc::new(AtomicU32::new(1));

        let cancel_dl = cancel.clone();
        let cancel_token_dl = cancel_token.clone();
        let total_dl = total.clone();
        let completed_dl = completed.clone();
        let speed_dl = speed.clone();
        let connections_dl = connections.clone();

        let total_ref = total.clone();
        let completed_ref = completed.clone();
        let gid_clone = gid.clone();

        let active_for_insert = active.clone();
        let gid_for_insert = gid.clone();
        let completion_handle = tokio::spawn(async move {
            let ad = ActiveDownload {
                cancel,
                cancel_token: cancel_token.clone(),
                total,
                completed,
                speed,
                connections,
            };
            active_for_insert.write().await.insert(gid_for_insert, ad);

            let download_result = super::ftp::run_ftp_download(
                &uri,
                &dir,
                &out,
                &merged_options,
                total_dl,
                completed_dl,
                speed_dl,
                cancel_dl,
                connections_dl,
                cancel_token_dl,
                global_limiter,
                task_speed_limiter,
            )
            .await;

            let mut tasks_guard = tasks.write().await;
            if let Some(task) = tasks_guard.iter_mut().find(|t| t.gid == gid_clone) {
                task.total_length = total_ref.load(Ordering::Relaxed);
                task.completed_length = completed_ref.load(Ordering::Relaxed);
                task.download_speed = 0;

                match download_result {
                    Ok(path) => {
                        task.status = TaskStatus::Complete;
                        task.files = vec![DownloadFile {
                            index: "1".to_string(),
                            path: path.to_string_lossy().to_string(),
                            length: task.total_length.to_string(),
                            completed_length: task.total_length.to_string(),
                            selected: "true".to_string(),
                            uris: task
                                .uris
                                .iter()
                                .map(|u| FileUri {
                                    uri: u.clone(),
                                    status: "used".to_string(),
                                })
                                .collect(),
                        }];
                        events.send(EngineEvent::DownloadComplete {
                            gid: gid_clone.clone(),
                        });
                    }
                    Err(e) => {
                        if e.contains("cancelled") {
                            // Don't overwrite Removed status from remove()
                            if task.status != TaskStatus::Removed {
                                task.status = TaskStatus::Paused;
                            }
                            events.send(EngineEvent::DownloadPause {
                                gid: gid_clone.clone(),
                            });
                        } else {
                            tracing::error!("[ftp] Download failed for {}: {}", gid_clone, e);
                            task.status = TaskStatus::Error;
                            task.error_code = Some("1".to_string());
                            task.error_message = Some(e);
                            events.send(EngineEvent::DownloadError {
                                gid: gid_clone.clone(),
                            });
                        }
                    }
                }
            }
            drop(tasks_guard);

            active.write().await.remove(&gid_clone);
        });

        drop(completion_handle);
    }

    /// Update progress for all active downloads
    /// Also starts waiting tasks if slots are available
    pub async fn update_progress(&self) {
        {
            let active = self.active_downloads.read().await;
            let mut tasks = self.tasks.write().await;

            for task in tasks.iter_mut() {
                if task.status != TaskStatus::Active {
                    continue;
                }
                if let Some(ad) = active.get(&task.gid) {
                    task.total_length = ad.total.load(Ordering::Relaxed);
                    task.completed_length = ad.completed.load(Ordering::Relaxed);
                    task.download_speed = ad.speed.load(Ordering::Relaxed);
                    task.connections = ad.connections.load(Ordering::Relaxed);
                    if let Some(f) = task.files.first_mut() {
                        f.length = task.total_length.to_string();
                        f.completed_length = task.completed_length.to_string();
                        // if it's still a raw URL, resolve to disk path
                        if looks_like_url(&f.path) {
                            let filename = if !task.out.is_empty() {
                                task.out.clone()
                            } else if let Some(uri) = task.uris.first() {
                                let name = http::infer_filename_from_uri(uri);
                                format!("{name}.part")
                            } else {
                                String::new()
                            };
                            if !filename.is_empty() {
                                let display = filename.strip_suffix(".part").unwrap_or(&filename);
                                f.path = format!("{}/{}", task.dir, display);
                            }
                        }
                    }
                }
            }

            // Update torrent tasks
            let te_guard = self.torrent_engine.read().await;
            let tid_guard = self.torrent_ids.read().await;
            let (keep_seeding, seed_time_minutes, seed_ratio) = {
                let opts = self.options.read().await;
                let st = opts.seed_time();
                (st > 0, st, opts.seed_ratio())
            };
            if let Some(ref te) = *te_guard {
                for task in tasks.iter_mut() {
                    if task.kind != TaskKind::Torrent || task.status != TaskStatus::Active {
                        continue;
                    }
                    if let Some(&tid) = tid_guard.get(&task.gid) {
                        if let Some(stats) = te.get_torrent_stats(tid) {
                            task.total_length = stats.total_bytes;
                            task.completed_length = stats.downloaded_bytes;
                            task.upload_length = stats.uploaded_bytes;
                            task.download_speed = stats.download_speed;
                            task.upload_speed = stats.upload_speed;
                            task.connections = stats.num_peers;

                            if task.bt_name.is_none() {
                                if let Some(ref name) = stats.name {
                                    task.bt_name = Some(name.clone());
                                }
                            }

                            // Populate file list from torrent metadata
                            if let Some(ref file_details) = stats.file_details {
                                let torrent_name = task.bt_name.as_deref().unwrap_or("");
                                let base_dir = if torrent_name.is_empty() {
                                    task.dir.clone()
                                } else if file_details.len() > 1 {
                                    // Multi-file: files are inside torrent folder
                                    format!("{}/{}", task.dir, torrent_name)
                                } else {
                                    task.dir.clone()
                                };

                                // Determine which files are selected (0-based indices)
                                let selected_indices: Option<std::collections::HashSet<usize>> =
                                    task.options.get("select-file")
                                        .and_then(|v| v.as_str())
                                        .and_then(|raw| {
                                            let raw = raw.trim();
                                            if raw.is_empty() { return None; }
                                            let set: std::collections::HashSet<usize> = raw
                                                .split(',')
                                                .filter_map(|s| s.trim().parse::<usize>().ok())
                                                .filter(|&i| i >= 1)
                                                .map(|i| i - 1) // 1-based to 0-based
                                                .collect();
                                            if set.is_empty() { None } else { Some(set) }
                                        });

                                let mut selected_total: u64 = 0;
                                let mut selected_completed: u64 = 0;

                                task.files = file_details
                                    .iter()
                                    .map(|fd| {
                                        let completed = stats
                                            .file_progress
                                            .get(fd.index)
                                            .copied()
                                            .unwrap_or(0);
                                        let is_selected = selected_indices
                                            .as_ref()
                                            .map_or(true, |set| set.contains(&fd.index));
                                        if is_selected {
                                            selected_total += fd.length;
                                            selected_completed += completed;
                                        }
                                        DownloadFile {
                                            index: (fd.index + 1).to_string(), // 1-based for compatibility
                                            path: format!("{}/{}", base_dir, fd.path),
                                            length: fd.length.to_string(),
                                            completed_length: completed.to_string(),
                                            selected: if is_selected { "true" } else { "false" }.to_string(),
                                            uris: Vec::new(),
                                        }
                                    })
                                    .collect();

                                // Override totals with selected-only sums
                                if selected_indices.is_some() {
                                    task.total_length = selected_total;
                                    task.completed_length = selected_completed;
                                }
                            } else if task.files.is_empty() {
                                // Fallback: metadata not yet available
                                if let Some(ref name) = stats.name {
                                    task.files = vec![DownloadFile {
                                        index: "1".to_string(),
                                        path: format!("{}/{}", task.dir, name),
                                        length: stats.total_bytes.to_string(),
                                        completed_length: stats.downloaded_bytes.to_string(),
                                        selected: "true".to_string(),
                                        uris: Vec::new(),
                                    }];
                                }
                            } else {
                                // Update progress for existing single-entry fallback
                                if let Some(f) = task.files.first_mut() {
                                    f.length = stats.total_bytes.to_string();
                                    f.completed_length = stats.downloaded_bytes.to_string();
                                }
                            }

                            if stats.is_finished && !task.seeder {
                                if keep_seeding {
                                    // Mark as seeder but keep Active so the torrent
                                    // continues uploading to peers
                                    task.seeder = true;
                                    task.seeding_since = std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .unwrap_or_default()
                                        .as_millis() as u64;
                                    task.download_speed = 0;
                                    self.events.send(EngineEvent::BtDownloadComplete {
                                        gid: task.gid.clone(),
                                    });
                                } else {
                                    task.status = TaskStatus::Complete;
                                    self.events.send(EngineEvent::BtDownloadComplete {
                                        gid: task.gid.clone(),
                                    });
                                    self.events.send(EngineEvent::DownloadComplete {
                                        gid: task.gid.clone(),
                                    });
                                }
                            }

                            // Check if seed time has elapsed or seed ratio reached
                            if task.seeder && task.seeding_since > 0 {
                                let mut should_stop = false;

                                // Check seed time limit
                                let seed_time_ms = seed_time_minutes * 60 * 1000;
                                if seed_time_ms > 0 {
                                    let now = std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .unwrap_or_default()
                                        .as_millis() as u64;
                                    if now - task.seeding_since >= seed_time_ms as u64 {
                                        should_stop = true;
                                    }
                                }

                                // Check seed ratio limit
                                if !should_stop && seed_ratio > 0.0 && task.total_length > 0 {
                                    let current_ratio = task.upload_length as f64 / task.total_length as f64;
                                    if current_ratio >= seed_ratio {
                                        should_stop = true;
                                    }
                                }

                                if should_stop {
                                    task.seeder = false;
                                    task.seeding_since = 0;
                                    task.status = TaskStatus::Complete;
                                    self.events.send(EngineEvent::DownloadComplete {
                                        gid: task.gid.clone(),
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
        // Start any waiting tasks if download slots are available
        self.try_start_next().await;
    }

    pub async fn pause(&self, gid: &str) -> Result<(), String> {
        // Cancel active HTTP download
        {
            let active = self.active_downloads.read().await;
            if let Some(ad) = active.get(gid) {
                ad.cancel.store(true, Ordering::Relaxed);
                ad.cancel_token.cancel();
            }
        }

        // Pause torrent
        {
            let tid_guard = self.torrent_ids.read().await;
            if let Some(&tid) = tid_guard.get(gid) {
                let te_guard = self.torrent_engine.read().await;
                if let Some(ref te) = *te_guard {
                    te.pause(tid).await.ok();
                }
            }
        }

        let mut tasks = self.tasks.write().await;
        if let Some(task) = tasks.iter_mut().find(|t| t.gid == gid) {
            if task.status == TaskStatus::Active || task.status == TaskStatus::Waiting {
                task.status = TaskStatus::Paused;
                task.download_speed = 0;
                task.upload_speed = 0;
                self.events.send(EngineEvent::DownloadPause {
                    gid: gid.to_string(),
                });
                return Ok(());
            }
        }
        Err(format!("Task {} not found or not active", gid))
    }

    pub async fn unpause(&self, gid: &str) -> Result<(), String> {
        let is_torrent;
        {
            let mut tasks = self.tasks.write().await;
            let task = tasks
                .iter_mut()
                .find(|t| t.gid == gid)
                .ok_or_else(|| format!("Task {} not found or not paused", gid))?;
            if task.status != TaskStatus::Paused {
                return Err(format!("Task {} not found or not paused", gid));
            }
            is_torrent = task.kind == TaskKind::Torrent;
            if is_torrent {
                task.status = TaskStatus::Active;
            } else {
                task.status = TaskStatus::Waiting;
            }
        }

        // Resume torrent in engine
        if is_torrent {
            let tid_guard = self.torrent_ids.read().await;
            if let Some(&tid) = tid_guard.get(gid) {
                let te_guard = self.torrent_engine.read().await;
                if let Some(ref te) = *te_guard {
                    te.unpause(tid).await.ok();
                }
            }
        } else {
            self.try_start_next().await;
        }

        self.events.send(EngineEvent::DownloadStart {
            gid: gid.to_string(),
        });
        Ok(())
    }

    pub async fn remove(&self, gid: &str) -> Result<(), String> {
        // Cancel any active download
        {
            let active = self.active_downloads.read().await;
            if let Some(ad) = active.get(gid) {
                ad.cancel.store(true, Ordering::Relaxed);
                ad.cancel_token.cancel();
            }
        }

        // Remove from torrent engine
        {
            let tid_guard = self.torrent_ids.read().await;
            if let Some(&tid) = tid_guard.get(gid) {
                let te_guard = self.torrent_engine.read().await;
                if let Some(ref te) = *te_guard {
                    te.remove(tid).await.ok();
                }
            }
        }
        self.torrent_ids.write().await.remove(gid);

        let mut tasks = self.tasks.write().await;
        if let Some(task) = tasks.iter_mut().find(|t| t.gid == gid) {
            task.status = TaskStatus::Removed;
            task.download_speed = 0;
            task.upload_speed = 0;
            self.events.send(EngineEvent::DownloadStop {
                gid: gid.to_string(),
            });
            return Ok(());
        }
        Err(format!("Task {} not found", gid))
    }

    pub async fn tell_status(&self, gid: &str, keys: &[String]) -> Result<Value, String> {
        let tasks = self.tasks.read().await;
        tasks
            .iter()
            .find(|t| t.gid == gid)
            .map(|t| t.to_rpc_status(keys))
            .ok_or_else(|| format!("GID {} not found", gid))
    }

    pub async fn tell_active(&self, keys: &[String]) -> Value {
        let tasks = self.tasks.read().await;
        let active: Vec<Value> = tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Active)
            .map(|t| t.to_rpc_status(keys))
            .collect();
        Value::Array(active)
    }

    pub async fn tell_waiting(&self, offset: i64, num: usize, keys: &[String]) -> Value {
        let tasks = self.tasks.read().await;
        let waiting: Vec<&DownloadTask> = tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Waiting || t.status == TaskStatus::Paused)
            .collect();

        let start = if offset >= 0 {
            offset as usize
        } else {
            waiting.len().saturating_sub((-offset) as usize)
        };

        let slice: Vec<Value> = waiting
            .iter()
            .skip(start)
            .take(num)
            .map(|t| t.to_rpc_status(keys))
            .collect();
        Value::Array(slice)
    }

    pub async fn tell_stopped(&self, offset: i64, num: usize, keys: &[String]) -> Value {
        let tasks = self.tasks.read().await;
        let stopped: Vec<&DownloadTask> = tasks
            .iter()
            .filter(|t| t.status.is_stopped())
            .collect();

        let start = if offset >= 0 {
            offset as usize
        } else {
            stopped.len().saturating_sub((-offset) as usize)
        };

        let slice: Vec<Value> = stopped
            .iter()
            .skip(start)
            .take(num)
            .map(|t| t.to_rpc_status(keys))
            .collect();
        Value::Array(slice)
    }

    pub async fn get_global_stat(&self) -> Value {
        let tasks = self.tasks.read().await;
        let mut num_active = 0u64;
        let mut num_waiting = 0u64;
        let mut num_stopped = 0u64;
        let mut dl_speed = 0u64;
        let mut ul_speed = 0u64;

        for task in tasks.iter() {
            match task.status {
                TaskStatus::Active => {
                    num_active += 1;
                    dl_speed += task.download_speed;
                    ul_speed += task.upload_speed;
                }
                TaskStatus::Waiting | TaskStatus::Paused => num_waiting += 1,
                _ => num_stopped += 1,
            }
        }

        serde_json::json!({
            "numActive": num_active.to_string(),
            "numWaiting": num_waiting.to_string(),
            "numStopped": num_stopped.to_string(),
            "numStoppedTotal": num_stopped.to_string(),
            "downloadSpeed": dl_speed.to_string(),
            "uploadSpeed": ul_speed.to_string(),
        })
    }

    pub async fn change_position(
        &self,
        gid: &str,
        pos: i64,
        how: &str,
    ) -> Result<Value, String> {
        let mut tasks = self.tasks.write().await;
        let waiting: Vec<usize> = tasks
            .iter()
            .enumerate()
            .filter(|(_, t)| t.status == TaskStatus::Waiting || t.status == TaskStatus::Paused)
            .map(|(i, _)| i)
            .collect();

        let current_waiting_pos = waiting
            .iter()
            .position(|&idx| tasks[idx].gid == gid)
            .ok_or_else(|| format!("GID {} not in waiting queue", gid))?;

        let target_waiting_pos = match how {
            "POS_SET" => pos.max(0) as usize,
            "POS_CUR" => (current_waiting_pos as i64 + pos).max(0) as usize,
            "POS_END" => {
                if pos >= 0 {
                    waiting.len().saturating_sub(1)
                } else {
                    (waiting.len() as i64 + pos).max(0) as usize
                }
            }
            _ => return Err("Invalid position mode".to_string()),
        };

        let target_waiting_pos = target_waiting_pos.min(waiting.len().saturating_sub(1));

        if current_waiting_pos != target_waiting_pos {
            let task_idx = waiting[current_waiting_pos];
            let task = tasks.remove(task_idx);

            // Recalculate target index in the full list
            let waiting_after_remove: Vec<usize> = tasks
                .iter()
                .enumerate()
                .filter(|(_, t)| {
                    t.status == TaskStatus::Waiting || t.status == TaskStatus::Paused
                })
                .map(|(i, _)| i)
                .collect();

            let insert_idx = if target_waiting_pos < waiting_after_remove.len() {
                waiting_after_remove[target_waiting_pos]
            } else {
                tasks.len()
            };

            tasks.insert(insert_idx, task);
        }

        Ok(Value::Number(serde_json::Number::from(target_waiting_pos as u64)))
    }

    /// Return GIDs of waiting/paused tasks that are in `filter`, preserving queue order
    pub async fn get_waiting_gids_in_order(
        &self,
        filter: &std::collections::HashSet<String>,
    ) -> Vec<String> {
        let tasks = self.tasks.read().await;
        tasks
            .iter()
            .filter(|t| {
                (t.status == TaskStatus::Waiting || t.status == TaskStatus::Paused)
                    && filter.contains(&t.gid)
            })
            .map(|t| t.gid.clone())
            .collect()
    }

    pub async fn change_option(&self, gid: &str, opts: Map<String, Value>) -> Result<(), String> {
        let mut tasks = self.tasks.write().await;
        if let Some(task) = tasks.iter_mut().find(|t| t.gid == gid) {
            // If seed-time is being set to 0, stop seeding immediately
            if let Some(v) = opts.get("seed-time") {
                let val = v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())).unwrap_or(0);
                if val == 0 && task.seeder {
                    // Only stop if seed-ratio is also 0 or absent
                    let ratio_zero = opts.get("seed-ratio")
                        .and_then(|r| r.as_f64().or_else(|| r.as_str().and_then(|s| s.parse().ok())))
                        .map_or(true, |r| r <= 0.0);
                    if ratio_zero {
                        task.seeder = false;
                        task.seeding_since = 0;
                        task.status = TaskStatus::Complete;
                        self.events.send(EngineEvent::DownloadComplete {
                            gid: task.gid.clone(),
                        });
                    }
                }
            }
            for (k, v) in opts {
                task.options.insert(k, v);
            }
            return Ok(());
        }
        Err(format!("GID {} not found", gid))
    }

    pub async fn change_global_option(&self, opts: Map<String, Value>) {
        // Update the global speed limiter if speed limits changed
        if let Some(v) = opts.get("max-overall-download-limit") {
            self.global_speed_limiter.set_limit(parse_speed_limit(v));
        }

        let mut options = self.options.write().await;
        for (k, v) in opts {
            options.set(k, v);
        }
    }

    pub async fn get_option(&self, gid: &str) -> Result<Value, String> {
        let tasks = self.tasks.read().await;
        tasks
            .iter()
            .find(|t| t.gid == gid)
            .map(|t| Value::Object(t.options.clone()))
            .ok_or_else(|| format!("GID {} not found", gid))
    }

    pub async fn get_global_option(&self) -> Value {
        Value::Object(self.options.read().await.global.clone())
    }

    pub async fn get_peers(&self, gid: &str) -> Value {
        let tasks = self.tasks.read().await;
        if let Some(task) = tasks.iter().find(|t| t.gid == gid) {
            let peers: Vec<Value> = task
                .peers
                .iter()
                .map(|p| serde_json::to_value(p).unwrap_or(Value::Null))
                .collect();
            Value::Array(peers)
        } else {
            Value::Array(Vec::new())
        }
    }

    pub async fn get_uris(&self, gid: &str) -> Result<Value, String> {
        let tasks = self.tasks.read().await;
        let task = tasks
            .iter()
            .find(|t| t.gid == gid)
            .ok_or_else(|| format!("GID {} not found", gid))?;
        // Prefer URIs from first file entry, fall back to task-level uris
        let uris: Vec<Value> = if let Some(file) = task.files.first() {
            if !file.uris.is_empty() {
                file.uris
                    .iter()
                    .map(|u| {
                        serde_json::json!({
                            "uri": u.uri,
                            "status": u.status,
                        })
                    })
                    .collect()
            } else {
                task.uris
                    .iter()
                    .enumerate()
                    .map(|(i, u)| {
                        serde_json::json!({
                            "uri": u,
                            "status": if i == 0 { "used" } else { "waiting" },
                        })
                    })
                    .collect()
            }
        } else {
            task.uris
                .iter()
                .enumerate()
                .map(|(i, u)| {
                    serde_json::json!({
                        "uri": u,
                        "status": if i == 0 { "used" } else { "waiting" },
                    })
                })
                .collect()
        };
        Ok(Value::Array(uris))
    }

    pub async fn get_files(&self, gid: &str) -> Result<Value, String> {
        let tasks = self.tasks.read().await;
        let task = tasks
            .iter()
            .find(|t| t.gid == gid)
            .ok_or_else(|| format!("GID {} not found", gid))?;
        let files: Vec<Value> = task
            .files
            .iter()
            .map(|f| serde_json::to_value(f).unwrap_or(Value::Null))
            .collect();
        Ok(Value::Array(files))
    }

    pub async fn get_servers(&self, gid: &str) -> Result<Value, String> {
        let tasks = self.tasks.read().await;
        let task = tasks
            .iter()
            .find(|t| t.gid == gid)
            .ok_or_else(|| format!("GID {} not found", gid))?;
        // connection state like aria2, return a minimal structure.
        let servers: Vec<Value> = task
            .files
            .iter()
            .map(|f| {
                let svrs: Vec<Value> = f
                    .uris
                    .iter()
                    .map(|u| {
                        serde_json::json!({
                            "uri": u.uri,
                            "currentUri": u.uri,
                            "downloadSpeed": "0",
                        })
                    })
                    .collect();
                serde_json::json!({
                    "index": f.index,
                    "servers": svrs,
                })
            })
            .collect();
        Ok(Value::Array(servers))
    }

    pub async fn save_session(&self) -> Result<(), String> {
        let tasks = self.tasks.read().await;
        self.session.save(&tasks)
    }

    pub async fn purge_download_result(&self) {
        let mut tasks = self.tasks.write().await;
        tasks.retain(|t| !t.status.is_stopped());
    }

    pub async fn remove_download_result(&self, gid: &str) -> Result<(), String> {
        let mut tasks = self.tasks.write().await;
        let len_before = tasks.len();
        tasks.retain(|t| !(t.gid == gid && t.status.is_stopped()));
        if tasks.len() < len_before {
            Ok(())
        } else {
            Err(format!("GID {} not found or not stopped", gid))
        }
    }

    pub async fn pause_all(&self) {
        let mut torrent_gids = Vec::new();
        {
            let mut tasks = self.tasks.write().await;
            for task in tasks.iter_mut() {
                if task.status == TaskStatus::Active || task.status == TaskStatus::Waiting {
                    if task.kind == TaskKind::Torrent {
                        torrent_gids.push(task.gid.clone());
                    }
                    task.status = TaskStatus::Paused;
                    task.download_speed = 0;
                    task.upload_speed = 0;
                    self.events.send(EngineEvent::DownloadPause {
                        gid: task.gid.clone(),
                    });
                }
            }
        }
        // Cancel all active HTTP downloads
        let active = self.active_downloads.read().await;
        for (_, ad) in active.iter() {
            ad.cancel.store(true, Ordering::Relaxed);
            ad.cancel_token.cancel();
        }
        drop(active);
        // Pause all active torrents
        let tid_guard = self.torrent_ids.read().await;
        let te_guard = self.torrent_engine.read().await;
        if let Some(ref te) = *te_guard {
            for gid in &torrent_gids {
                if let Some(&tid) = tid_guard.get(gid) {
                    te.pause(tid).await.ok();
                }
            }
        }
    }

    pub async fn unpause_all(&self) {
        let mut torrent_gids = Vec::new();
        {
            let mut tasks = self.tasks.write().await;
            for task in tasks.iter_mut() {
                if task.status == TaskStatus::Paused {
                    if task.kind == TaskKind::Torrent {
                        task.status = TaskStatus::Active;
                        torrent_gids.push(task.gid.clone());
                    } else {
                        task.status = TaskStatus::Waiting;
                    }
                    self.events.send(EngineEvent::DownloadStart {
                        gid: task.gid.clone(),
                    });
                }
            }
        }
        // Resume torrents in engine
        let tid_guard = self.torrent_ids.read().await;
        let te_guard = self.torrent_engine.read().await;
        if let Some(ref te) = *te_guard {
            for gid in &torrent_gids {
                if let Some(&tid) = tid_guard.get(gid) {
                    te.unpause(tid).await.ok();
                }
            }
        }
        drop(te_guard);
        drop(tid_guard);
        self.try_start_next().await;
    }

    pub async fn shutdown(&self) {
        // Cancel all active downloads
        let active = self.active_downloads.read().await;
        for (_, ad) in active.iter() {
            ad.cancel.store(true, Ordering::Relaxed);
        }
        drop(active);

        // Save session
        if let Err(e) = self.save_session().await {
            log::error!("Failed to save session on shutdown: {}", e);
        }

        // Shut down torrent engine
        let mut te_guard = self.torrent_engine.write().await;
        if let Some(mut te) = te_guard.take() {
            te.shutdown().await;
        }
    }
}

fn infer_m3u8_output_name(uri: &str) -> String {
    let path = uri.split('?').next().unwrap_or(uri);
    let path = path.split('#').next().unwrap_or(path);
    let name = path.rsplit('/').next().unwrap_or("download");

    if let Some(stem) = name
        .strip_suffix(".m3u8")
        .or_else(|| name.strip_suffix(".m3u"))
    {
        format!("{stem}.ts")
    } else if name.is_empty() {
        "download.ts".to_string()
    } else {
        format!("{name}.ts")
    }
}

fn looks_like_url(path: &str) -> bool {
    path.starts_with("http://")
        || path.starts_with("https://")
        || path.starts_with("ftp://")
        || path.starts_with("ed2k://")
}
