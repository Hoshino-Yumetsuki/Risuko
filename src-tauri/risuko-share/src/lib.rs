//! Peer-to-peer encrypted file sharing for Risuko

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use anyhow::{Context, Result};
use iroh::{
    address_lookup::MemoryLookup,
    endpoint::{presets, TransportAddrUsage},
    protocol::Router,
    Endpoint, EndpointId, RelayMode,
};
use iroh_blobs::{
    api::{downloader::DownloadProgressItem, downloader::Shuffled, Store},
    format::collection::Collection,
    provider::events::{
        ConnectMode, EventMask, EventSender, ProviderMessage, RequestMode, RequestUpdate,
    },
    store::{fs::FsStore, mem::MemStore},
    ticket::BlobTicket,
    BlobFormat, BlobsProtocol, Hash, HashAndFormat,
};
use n0_future::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;

struct SendProgressTracker {
    total: u64,
    finished: u64,
    current_end: u64,
}

impl SendProgressTracker {
    fn transferred(&self) -> u64 {
        (self.finished + self.current_end).min(self.total)
    }
}

fn watch_send_request(
    mut rx: irpc::channel::mpsc::Receiver<RequestUpdate>,
    id: String,
    events: UnboundedSender<ShareEnvelope>,
    stop: Arc<AtomicBool>,
    tracker: Arc<Mutex<SendProgressTracker>>,
) {
    tokio::spawn(async move {
        while let Ok(Some(update)) = rx.recv().await {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            let transferred = {
                let mut state = tracker.lock().unwrap();
                match update {
                    RequestUpdate::Started { .. } => {
                        state.current_end = 0;
                    }
                    RequestUpdate::Progress(progress) => {
                        state.current_end = progress.end_offset;
                    }
                    RequestUpdate::Completed(done) => {
                        state.finished += done.stats.payload_bytes_sent;
                        state.current_end = 0;
                    }
                    RequestUpdate::Aborted(_) => break,
                }
                state.transferred()
            };
            let _ = events.send(ShareEnvelope {
                id: id.clone(),
                event: ShareEvent::Progress {
                    transferred,
                    total: tracker.lock().unwrap().total,
                },
            });
        }
    });
}

fn begin_send_transfer(
    events: &UnboundedSender<ShareEnvelope>,
    id: &str,
    peer: Option<EndpointId>,
    endpoint: Endpoint,
    transferring: &mut bool,
    path_task: &mut Option<tokio::task::JoinHandle<()>>,
    stop: Arc<AtomicBool>,
) {
    if *transferring {
        return;
    }
    *transferring = true;
    let _ = events.send(ShareEnvelope {
        id: id.to_string(),
        event: ShareEvent::Connected,
    });
    if let Some(remote) = peer {
        *path_task = Some(spawn_path_poll(
            endpoint,
            remote,
            id.to_string(),
            events.clone(),
            stop,
        ));
    }
}

/// Interval for polling the active connection path
const PATH_POLL_INTERVAL: Duration = Duration::from_millis(700);
const ONLINE_TIMEOUT: Duration = Duration::from_secs(6);

/// Bind an iroh endpoint with n0 relays but WITHOUT DNS-based discovery
async fn bind_endpoint() -> Result<Endpoint> {
    Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Default)
        .bind()
        .await
        .context("failed to bind endpoint")
}

/// Bring an endpoint online
fn spawn_online_warmup(endpoint: Endpoint) {
    tokio::spawn(async move {
        endpoint.online().await;
    });
}

/// Metadata about a single file in a share
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileMeta {
    pub name: String,
    pub size: u64,
}

/// The kind of network path a live transfer is using
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PathKind {
    Direct,
    Relay,
    Mixed,
    /// Not yet determined
    Unknown,
}

/// An event about a transfer, tagged with the transfer id by [`ShareEnvelope`]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ShareEvent {
    Connected,
    Path {
        path: PathKind,
    },
    Progress {
        transferred: u64,
        total: u64,
    },
    Completed,
    Terminated,
    /// The transfer failed.
    Error {
        message: String,
    },
}

/// A [`ShareEvent`] tagged with its transfer id, sent to the host application.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareEnvelope {
    pub id: String,
    #[serde(flatten)]
    pub event: ShareEvent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendInfo {
    pub ticket: String,
    pub files: Vec<FileMeta>,
}

struct ActiveTransfer {
    /// Kept alive so the endpoint stays
    _router: Router,
    store: Store,
    blobs_dir: PathBuf,
    stop: Arc<AtomicBool>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl ActiveTransfer {
    async fn shutdown(self) {
        self.stop.store(true, Ordering::Relaxed);
        for task in self.tasks {
            task.abort();
        }
        self._router.shutdown().await.ok();
        let _ = self.store.shutdown().await;
        let _ = tokio::fs::remove_dir_all(&self.blobs_dir).await;
    }
}

struct ReceiveHandle {
    stop: Arc<AtomicBool>,
    task: tokio::task::JoinHandle<()>,
}

/// Manages all active P2P transfers and forwards their events to the host
pub struct ShareManager {
    data_dir: PathBuf,
    events: UnboundedSender<ShareEnvelope>,
    active: Arc<Mutex<HashMap<String, ActiveTransfer>>>,
    receives: Arc<Mutex<HashMap<String, ReceiveHandle>>>,
}

impl ShareManager {
    /// Create a manager
    pub fn new(data_dir: PathBuf, events: UnboundedSender<ShareEnvelope>) -> Self {
        Self {
            data_dir,
            events,
            active: Arc::new(Mutex::new(HashMap::new())),
            receives: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Prepare a send
    pub async fn start_send(&self, id: String, paths: Vec<PathBuf>) -> Result<SendInfo> {
        anyhow::ensure!(!paths.is_empty(), "no files selected");

        let blobs_dir = self.data_dir.join(format!("send-{id}"));
        tokio::fs::create_dir_all(&blobs_dir).await.ok();

        let mask = EventMask {
            connected: ConnectMode::Notify,
            get: RequestMode::NotifyLog,
            get_many: RequestMode::NotifyLog,
            ..EventMask::DEFAULT
        };
        let (event_tx, mut event_rx) = EventSender::channel(64, mask);

        let store: Store = FsStore::load(&blobs_dir)
            .await
            .context("failed to open blob store")?
            .into();

        // Expand the selection
        let mut entries: Vec<(String, PathBuf)> = Vec::new();
        let mut has_dir = false;
        for path in &paths {
            let abs = std::path::absolute(path)?;
            let meta = std::fs::metadata(&abs).context("failed to stat selection")?;
            if meta.is_dir() {
                has_dir = true;
                collect_dir(&abs, &file_name(&abs), &mut entries)?;
            } else {
                entries.push((file_name(&abs), abs));
            }
        }
        anyhow::ensure!(!entries.is_empty(), "no files to send");

        let mut files = Vec::with_capacity(entries.len());
        for (name, abs) in &entries {
            let size = std::fs::metadata(abs).map(|m| m.len()).unwrap_or(0);
            files.push(FileMeta {
                name: name.clone(),
                size,
            });
        }

        let ticket_hash;
        let ticket_format;

        // A lone single file transfers as a raw blob
        if entries.len() == 1 && !has_dir {
            let tag = store.blobs().add_path(&entries[0].1).await?;
            ticket_hash = tag.hash;
            ticket_format = tag.format;
        } else {
            let mut items: Vec<(String, Hash)> = Vec::with_capacity(entries.len());
            for (name, abs) in &entries {
                let tag = store.blobs().add_path(abs).await?;
                items.push((name.clone(), tag.hash));
            }
            let collection = Collection::from_iter(items);
            let tt = collection.store(&store).await?;
            store.tags().create(tt.hash_and_format()).await?;
            ticket_hash = tt.hash();
            ticket_format = BlobFormat::HashSeq;
        }

        let endpoint = bind_endpoint().await?;
        // Wait (bounded) for a relay so the ticket embeds reachable addresses
        let _ = tokio::time::timeout(ONLINE_TIMEOUT, endpoint.online()).await;
        let addr = endpoint.addr();

        let blobs = BlobsProtocol::new(&store, Some(event_tx));
        let router = Router::builder(endpoint)
            .accept(iroh_blobs::ALPN, blobs)
            .spawn();

        let ticket = BlobTicket::new(addr, ticket_hash, ticket_format).to_string();

        let stop = Arc::new(AtomicBool::new(false));

        let events = self.events.clone();
        let active = self.active.clone();
        let endpoint_for_events = router.endpoint().clone();
        let id_for_events = id.clone();
        let stop_for_events = stop.clone();
        let total_bytes: u64 = files.iter().map(|f| f.size).sum();
        let progress_tracker = Arc::new(Mutex::new(SendProgressTracker {
            total: total_bytes,
            finished: 0,
            current_end: 0,
        }));
        let event_task = tokio::spawn(async move {
            let mut peer: Option<EndpointId> = None;
            let mut transferring = false;
            let mut completed = false;
            let mut path_task: Option<tokio::task::JoinHandle<()>> = None;

            while let Some(msg) = event_rx.recv().await {
                if stop_for_events.load(Ordering::Relaxed) {
                    break;
                }
                match msg {
                    ProviderMessage::ClientConnected(m) => {
                        peer = m.endpoint_id;
                    }
                    ProviderMessage::ClientConnectedNotify(m) => {
                        peer = m.endpoint_id;
                    }
                    ProviderMessage::GetRequestReceivedNotify(m) => {
                        begin_send_transfer(
                            &events,
                            &id_for_events,
                            peer,
                            endpoint_for_events.clone(),
                            &mut transferring,
                            &mut path_task,
                            stop_for_events.clone(),
                        );
                        watch_send_request(
                            m.rx,
                            id_for_events.clone(),
                            events.clone(),
                            stop_for_events.clone(),
                            progress_tracker.clone(),
                        );
                    }
                    ProviderMessage::GetManyRequestReceivedNotify(m) => {
                        begin_send_transfer(
                            &events,
                            &id_for_events,
                            peer,
                            endpoint_for_events.clone(),
                            &mut transferring,
                            &mut path_task,
                            stop_for_events.clone(),
                        );
                        watch_send_request(
                            m.rx,
                            id_for_events.clone(),
                            events.clone(),
                            stop_for_events.clone(),
                            progress_tracker.clone(),
                        );
                    }
                    ProviderMessage::ConnectionClosed(_) if transferring && !completed => {
                        completed = true;
                        if stop_for_events.load(Ordering::Relaxed) {
                            // User cancelled
                        } else {
                            let transferred = progress_tracker.lock().unwrap().transferred();
                            if total_bytes > 0 && transferred >= total_bytes {
                                let _ = events.send(ShareEnvelope {
                                    id: id_for_events.clone(),
                                    event: ShareEvent::Progress {
                                        transferred: total_bytes,
                                        total: total_bytes,
                                    },
                                });
                                let _ = events.send(ShareEnvelope {
                                    id: id_for_events.clone(),
                                    event: ShareEvent::Completed,
                                });
                            } else {
                                let _ = events.send(ShareEnvelope {
                                    id: id_for_events.clone(),
                                    event: ShareEvent::Terminated,
                                });
                            }
                        }
                    }
                    _ => {}
                }
            }

            if let Some(t) = path_task {
                t.abort();
            }
            if completed {
                let transfer = active.lock().unwrap().remove(&id_for_events);
                if let Some(transfer) = transfer {
                    transfer.shutdown().await;
                }
            }
        });

        let transfer = ActiveTransfer {
            _router: router,
            store,
            blobs_dir,
            stop,
            tasks: vec![event_task],
        };
        self.active.lock().unwrap().insert(id, transfer);

        Ok(SendInfo { ticket, files })
    }

    /// Start receiving from a ticket
    pub fn start_receive(
        &self,
        id: String,
        ticket: String,
        dest_dir: PathBuf,
        files: Vec<FileMeta>,
    ) -> Result<()> {
        let blobs_dir = self.data_dir.join(format!("recv-{id}"));
        let stop = Arc::new(AtomicBool::new(false));
        let events = self.events.clone();
        let active = self.active.clone();

        let stop_for_task = stop.clone();
        let id_for_task = id.clone();
        let dir_for_task = blobs_dir.clone();

        let handle = tokio::spawn(async move {
            let result = run_receive(
                &dir_for_task,
                &ticket,
                &dest_dir,
                &files,
                &id_for_task,
                &events,
                stop_for_task,
            )
            .await;

            match result {
                Ok(()) => {
                    let _ = events.send(ShareEnvelope {
                        id: id_for_task.clone(),
                        event: ShareEvent::Completed,
                    });
                }
                Err(err) => {
                    let message = err.to_string();
                    if message == "cancelled" {
                        return;
                    }
                    let _ = events.send(ShareEnvelope {
                        id: id_for_task.clone(),
                        event: ShareEvent::Error { message },
                    });
                }
            }

            let transfer = active.lock().unwrap().remove(&id_for_task);
            if let Some(transfer) = transfer {
                transfer.shutdown().await;
            }
        });
        self.receives
            .lock()
            .unwrap()
            .insert(id, ReceiveHandle { stop, task: handle });
        Ok(())
    }

    pub async fn cancel(&self, id: &str) {
        let transfer = self.active.lock().unwrap().remove(id);
        if let Some(transfer) = transfer {
            transfer.shutdown().await;
        }
        let recv = self.receives.lock().unwrap().remove(id);
        if let Some(recv) = recv {
            recv.stop.store(true, Ordering::Relaxed);
            recv.task.abort();
            let _ = tokio::fs::remove_dir_all(self.data_dir.join(format!("recv-{id}"))).await;
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_receive(
    _blobs_dir: &Path,
    ticket_str: &str,
    dest_dir: &Path,
    files: &[FileMeta],
    id: &str,
    events: &UnboundedSender<ShareEnvelope>,
    stop: Arc<AtomicBool>,
) -> Result<()> {
    tokio::fs::create_dir_all(dest_dir).await.ok();

    let ticket: BlobTicket = ticket_str.parse().context("invalid share ticket")?;
    let provider = ticket.addr().id;
    let hash = ticket.hash();
    let format = ticket.format();
    let total: u64 = files.iter().map(|f| f.size).sum();

    let store: Store = MemStore::new().into();

    // Seed the sender's addresses
    let lookup = MemoryLookup::new();
    lookup.add_endpoint_info(ticket.addr().clone());
    let endpoint = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Default)
        .address_lookup(lookup)
        .bind()
        .await
        .context("failed to bind endpoint")?;
    spawn_online_warmup(endpoint.clone());

    let path_handle = spawn_path_poll(
        endpoint.clone(),
        provider,
        id.to_string(),
        events.clone(),
        stop.clone(),
    );

    let _ = events.send(ShareEnvelope {
        id: id.to_string(),
        event: ShareEvent::Connected,
    });

    let request = match format {
        BlobFormat::Raw => HashAndFormat::raw(hash),
        _ => HashAndFormat::hash_seq(hash),
    };

    let downloader = store.downloader(&endpoint);
    let mut stream = downloader
        .download(request, Shuffled::new(vec![provider]))
        .stream()
        .await?;

    while let Some(item) = stream.next().await {
        if stop.load(Ordering::Relaxed) {
            path_handle.abort();
            anyhow::bail!("cancelled");
        }
        match item {
            DownloadProgressItem::Progress(offset) => {
                let _ = events.send(ShareEnvelope {
                    id: id.to_string(),
                    event: ShareEvent::Progress {
                        transferred: offset,
                        total,
                    },
                });
            }
            DownloadProgressItem::Error(err) => {
                path_handle.abort();
                anyhow::bail!("download failed: {err}");
            }
            DownloadProgressItem::DownloadError => {
                path_handle.abort();
                anyhow::bail!("download failed");
            }
            _ => {}
        }
    }
    path_handle.abort();

    // Export downloaded blob(s) to the destination directory
    match format {
        BlobFormat::Raw => {
            let name = files.first().map(|f| f.name.as_str()).unwrap_or("");
            let target = dedupe_path(dest_dir.join(sanitize_rel_path(name))).await;
            if let Some(parent) = target.parent() {
                tokio::fs::create_dir_all(parent).await.ok();
            }
            store.blobs().export(hash, target).await?;
        }
        _ => {
            let collection = Collection::load(hash, &store).await?;
            for (name, child) in collection.iter() {
                let target = dest_dir.join(sanitize_rel_path(name));
                if let Some(parent) = target.parent() {
                    tokio::fs::create_dir_all(parent).await.ok();
                }
                let target = dedupe_path(target).await;
                store.blobs().export(*child, target).await?;
            }
        }
    }

    let _ = store.shutdown().await;
    Ok(())
}

/// Poll the active connection path for a remote and emit [`ShareEvent::Path`]
fn spawn_path_poll(
    endpoint: Endpoint,
    remote: EndpointId,
    id: String,
    events: UnboundedSender<ShareEnvelope>,
    stop: Arc<AtomicBool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut last = PathKind::Unknown;
        loop {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            if let Some(info) = endpoint.remote_info(remote).await {
                let kind = classify_path(&info);
                if kind != PathKind::Unknown && kind != last {
                    last = kind;
                    let _ = events.send(ShareEnvelope {
                        id: id.clone(),
                        event: ShareEvent::Path { path: kind },
                    });
                }
            }
            tokio::time::sleep(PATH_POLL_INTERVAL).await;
        }
    })
}

/// Classify a remote's active transport addresses as direct, relay, or mixed.
fn classify_path(info: &iroh::endpoint::RemoteInfo) -> PathKind {
    let mut direct = false;
    let mut relay = false;
    for addr in info.addrs() {
        if !matches!(addr.usage(), TransportAddrUsage::Active) {
            continue;
        }
        if addr.addr().is_ip() {
            direct = true;
        }
        if addr.addr().is_relay() {
            relay = true;
        }
    }
    match (direct, relay) {
        (true, true) => PathKind::Mixed,
        (true, false) => PathKind::Direct,
        (false, true) => PathKind::Relay,
        (false, false) => PathKind::Unknown,
    }
}

/// Recursively collect files under `dir`
fn collect_dir(dir: &Path, prefix: &str, out: &mut Vec<(String, PathBuf)>) -> Result<()> {
    let read = std::fs::read_dir(dir).with_context(|| format!("failed to read {dir:?}"))?;
    for entry in read {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name.is_empty() {
            continue;
        }
        let rel = format!("{prefix}/{name}");
        if file_type.is_dir() {
            collect_dir(&entry.path(), &rel, out)?;
        } else if file_type.is_file() {
            out.push((rel, entry.path()));
        }
        // symlinks (file_type.is_symlink()) are intentionally skipped
    }
    Ok(())
}

/// If `target` already exists, append ` (1)`, ` (2)`, ... before the extension
/// until a free path is found
async fn dedupe_path(target: PathBuf) -> PathBuf {
    if !tokio::fs::try_exists(&target).await.unwrap_or(false) {
        return target;
    }
    let parent = target.parent().map(Path::to_path_buf).unwrap_or_default();
    let stem = target
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    let ext = target.extension().map(|s| s.to_string_lossy().to_string());
    for n in 1..=9999 {
        let name = match &ext {
            Some(ext) => format!("{stem} ({n}).{ext}"),
            None => format!("{stem} ({n})"),
        };
        let candidate = parent.join(name);
        if !tokio::fs::try_exists(&candidate).await.unwrap_or(false) {
            return candidate;
        }
    }
    target
}

/// Extract a safe file name (final path component only) from a path
fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|s| s.to_string_lossy().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "file".to_string())
}

/// Convert a peer-supplied name into a safe *relative* path. Strips empty,
/// `.`, `..`, and absolute/drive components to prevent directory traversal,
/// while preserving legitimate subdirectories
fn sanitize_rel_path(name: &str) -> PathBuf {
    let mut out = PathBuf::new();
    for component in name.split(['/', '\\']) {
        let component = component.trim();
        if component.is_empty() || component == "." || component == ".." || component.contains(':')
        {
            continue;
        }
        out.push(component);
    }
    if out.as_os_str().is_empty() {
        out.push("risuko-received");
    }
    out
}
