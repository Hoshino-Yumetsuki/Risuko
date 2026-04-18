//! Session: orchestrates all torrents, the TCP listener, and persistence

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use bytes::Bytes;
use parking_lot::Mutex;
use tokio::net::TcpListener;
use tokio::sync::mpsc;

use super::api::TorrentIdOrHash;
use super::core::metainfo::{parse_torrent, FileDetails};
use super::core::{generate_peer_id, Id20, Lengths};
use super::peer::{PeerCommand, PeerEvent};
use super::torrent::{spawn as spawn_torrent, ManagedTorrent, TorrentCommand, TorrentInit};

#[derive(Clone, Debug)]
pub enum SessionPersistenceConfig {
    Json { folder: Option<PathBuf> },
}

#[derive(Clone, Debug, Default)]
pub struct ListenerOptions {
    pub listen_addr: Option<SocketAddr>,
    pub enable_upnp_port_forwarding: bool,
    /// Optional override for UPnP mapping lease (default 300s when UPnP is
    /// enabled). Only consulted if `enable_upnp_port_forwarding` is true.
    pub upnp_lease: Option<std::time::Duration>,
    /// Also bind an IPv6 TCP listener on the same port. Opt-in because many
    /// hosts lack global v6 connectivity; a failed v6 bind is logged and
    /// ignored rather than aborting session startup.
    pub listen_ipv6: bool,
}

#[derive(Clone, Debug)]
pub struct SessionOptions {
    pub disable_dht: bool,
    pub disable_dht_persistence: bool,
    pub dht_config: Option<super::dht::DhtConfig>,
    pub listen: Option<ListenerOptions>,
    pub fastresume: bool,
    pub persistence: Option<SessionPersistenceConfig>,
    /// Maximum concurrent chunk requests per peer. Higher values improve
    /// throughput on high-latency links at the cost of more memory per peer
    /// `None` uses the crate default (128).
    pub max_outstanding_requests_per_peer: Option<usize>,
    /// Maximum simultaneous peer connections per torrent
    /// `None` uses the crate default (100)
    pub max_peers_per_torrent: Option<usize>,
    /// Disable BEP-14 Local Service Discovery. Defaults to enabled; some
    /// hosts (CI runners, strict corporate networks) cannot bind the
    /// multicast group and will warn-and-continue regardless.
    pub disable_local_service_discovery: bool,
    /// BEP-8 Message Stream Encryption (MSE/PE) policy for peer connections.
    /// Defaults to `Prefer`, which tries MSE first with plaintext fallback.
    pub encryption: super::peer::EncryptionPolicy,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            disable_dht: false,
            disable_dht_persistence: false,
            dht_config: None,
            listen: None,
            fastresume: false,
            persistence: None,
            max_outstanding_requests_per_peer: None,
            max_peers_per_torrent: None,
            disable_local_service_discovery: false,
            encryption: super::peer::EncryptionPolicy::default(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct AddTorrentOptions {
    pub output_folder: Option<String>,
    pub overwrite: bool,
    pub trackers: Option<Vec<String>>,
    pub only_files: Option<Vec<usize>>,
    pub list_only: bool,
}

pub enum AddTorrent {
    TorrentFileBytes(Bytes),
    Url(String),
}

pub enum AddTorrentResponse {
    Added(usize, Arc<ManagedTorrent>),
    AlreadyManaged(usize, Arc<ManagedTorrent>),
    ListOnly(ListOnlyResponse),
}

pub struct ListOnlyResponse {
    pub info: super::core::ValidatedTorrentMetaV1Info,
    pub files: Vec<FileDetails>,
}

pub struct Session {
    output_dir: PathBuf,
    opts: SessionOptions,
    peer_id: Id20,
    listen_port: u16,
    inner: Mutex<SessionInner>,
    accept_handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
    accept6_handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
    upnp_handle: Mutex<Option<super::upnp::UpnpHandle>>,
    lsd: Mutex<Option<Arc<super::lsd::LocalServiceDiscovery>>>,
    lsd_router_handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(h) = self.accept_handle.lock().take() {
            h.abort();
        }
        if let Some(h) = self.accept6_handle.lock().take() {
            h.abort();
        }
        if let Some(h) = self.lsd_router_handle.lock().take() {
            h.abort();
        }
        // Dropping the UpnpHandle / LSD service triggers their cleanup.
        let _ = self.upnp_handle.lock().take();
        let _ = self.lsd.lock().take();
    }
}

struct SessionInner {
    torrents: HashMap<usize, Arc<ManagedTorrent>>,
    by_hash: HashMap<Id20, usize>,
    next_id: usize,
}

impl Session {
    pub async fn new_with_opts(
        output_dir: PathBuf,
        opts: SessionOptions,
    ) -> std::io::Result<Arc<Self>> {
        std::fs::create_dir_all(&output_dir)?;
        let peer_id = generate_peer_id();

        let listen_addr = opts
            .listen
            .as_ref()
            .and_then(|l| l.listen_addr)
            .unwrap_or_else(|| "0.0.0.0:0".parse().unwrap());
        let listener = TcpListener::bind(listen_addr).await?;
        let local_port = listener.local_addr()?.port();
        log::info!("session listening on port {local_port}");

        let mut upnp_handle: Option<super::upnp::UpnpHandle> = None;
        if opts
            .listen
            .as_ref()
            .map(|l| l.enable_upnp_port_forwarding)
            .unwrap_or(false)
        {
            let lease = opts
                .listen
                .as_ref()
                .and_then(|l| l.upnp_lease)
                .unwrap_or(std::time::Duration::from_secs(300));
            let fwd = super::upnp::UpnpPortForwarder::new(
                vec![
                    (local_port, super::upnp::MapProto::Tcp),
                ],
                super::upnp::UpnpOptions {
                    lease,
                    ..Default::default()
                },
            );
            upnp_handle = Some(fwd.spawn());
        }

        let session = Arc::new(Self {
            output_dir,
            opts,
            peer_id,
            listen_port: local_port,
            inner: Mutex::new(SessionInner {
                torrents: HashMap::new(),
                by_hash: HashMap::new(),
                next_id: 1,
            }),
            accept_handle: Mutex::new(None),
            accept6_handle: Mutex::new(None),
            upnp_handle: Mutex::new(upnp_handle),
            lsd: Mutex::new(None),
            lsd_router_handle: Mutex::new(None),
        });

        if !session.opts.disable_local_service_discovery {
            match super::lsd::LocalServiceDiscovery::spawn(local_port, Vec::new()) {
                Ok((svc, mut rx)) => {
                    let weak = Arc::downgrade(&session);
                    let router = tokio::spawn(async move {
                        while let Some((ih, addr)) = rx.recv().await {
                            let Some(s) = weak.upgrade() else {
                                return;
                            };
                            let _ = s.add_peer(ih, addr).await;
                        }
                    });
                    *session.lsd.lock() = Some(svc);
                    *session.lsd_router_handle.lock() = Some(router);
                }
                Err(e) => log::warn!("lsd: not started: {e}"),
            }
        }

        // Hold a Weak reference inside the accept loop so the loop does not
        // keep the session alive. When the last external Arc is dropped the
        // session's Drop aborts the spawned task via `accept_handle`.
        let weak = Arc::downgrade(&session);
        let accept_handle = tokio::spawn(run_accept_loop(listener, weak));
        *session.accept_handle.lock() = Some(accept_handle);

        // Optional v6 listener on the same port. Failure to bind is not
        // fatal: we simply log and continue with v4 only.
        let listen_v6 = session
            .opts
            .listen
            .as_ref()
            .map(|l| l.listen_ipv6)
            .unwrap_or(false);
        if listen_v6 {
            let v6_addr: SocketAddr = (std::net::Ipv6Addr::UNSPECIFIED, local_port).into();
            match bind_v6_listener(v6_addr) {
                Ok(std_listener) => {
                    match TcpListener::from_std(std_listener) {
                        Ok(listener6) => {
                            log::info!("session v6 listening on port {local_port}");
                            let weak6 = Arc::downgrade(&session);
                            let accept6 = tokio::spawn(run_accept_loop(listener6, weak6));
                            *session.accept6_handle.lock() = Some(accept6);
                        }
                        Err(e) => log::warn!("session v6 tokio convert failed: {e}"),
                    }
                }
                Err(e) => log::warn!("session v6 bind failed: {e}"),
            }
        }

        Ok(session)
    }

    async fn route_inbound_peer(
        self: &Arc<Self>,
        addr: SocketAddr,
        cmd_tx: mpsc::Sender<PeerCommand>,
        event_rx: mpsc::Receiver<PeerEvent>,
    ) {
        let mut event_rx = event_rx;
        let Some(first) = event_rx.recv().await else {
            return;
        };
        let info_hash = match &first {
            PeerEvent::Handshook { info_hash, .. } => *info_hash,
            _ => return,
        };
        let target = {
            let inner = self.inner.lock();
            inner
                .by_hash
                .get(&info_hash)
                .and_then(|id| inner.torrents.get(id).cloned())
        };
        let Some(t) = target else {
            // Peer handshook for a torrent that is no longer managed, close
            let _ = cmd_tx.send(PeerCommand::Disconnect).await;
            return;
        };
        let (fwd_tx, fwd_rx) = mpsc::channel(64);
        let _ = fwd_tx.send(first).await;
        tokio::spawn(async move {
            while let Some(ev) = event_rx.recv().await {
                if fwd_tx.send(ev).await.is_err() {
                    break;
                }
            }
        });
        let _ = t
            .cmd_tx()
            .send(TorrentCommand::AddInboundPeer {
                addr,
                cmd_tx,
                event_rx: fwd_rx,
            })
            .await;
    }

    pub fn listen_port(&self) -> u16 {
        self.listen_port
    }

    pub async fn add_torrent(
        self: &Arc<Self>,
        which: AddTorrent,
        opts: Option<AddTorrentOptions>,
    ) -> Result<AddTorrentResponse, String> {
        let opts = opts.unwrap_or_default();
        match which {
            AddTorrent::TorrentFileBytes(bytes) => {
                let meta = parse_torrent(&bytes).map_err(|e| format!("parse torrent: {e}"))?;
                self.add_from_meta(meta, opts).await
            }
            AddTorrent::Url(url) => {
                let extra_trackers = opts.trackers.clone().unwrap_or_default();
                let resolved = super::magnet::resolve(
                    &url,
                    &extra_trackers,
                    std::time::Duration::from_secs(120),
                    self.opts.encryption,
                )
                .await?;
                let torrent_bytes =
                    super::magnet::synth_torrent_bytes(&resolved.info_bytes, &resolved.trackers);
                let meta = parse_torrent(&torrent_bytes)
                    .map_err(|e| format!("parse synthesized torrent: {e}"))?;
                self.add_from_meta(meta, opts).await
            }
        }
    }

    pub async fn add_from_meta(
        self: &Arc<Self>,
        meta: super::core::TorrentMeta,
        opts: AddTorrentOptions,
    ) -> Result<AddTorrentResponse, String> {
        let info = meta.info.clone();
        if opts.list_only {
            let files: Vec<FileDetails> = info.iter_file_details().collect();
            return Ok(AddTorrentResponse::ListOnly(ListOnlyResponse {
                info,
                files,
            }));
        }

        {
            let inner = self.inner.lock();
            if let Some(&id) = inner.by_hash.get(&meta.info_hash) {
                if let Some(t) = inner.torrents.get(&id).cloned() {
                    return Ok(AddTorrentResponse::AlreadyManaged(id, t));
                }
            }
        }

        let root_dir = opts
            .output_folder
            .map(PathBuf::from)
            .unwrap_or_else(|| self.output_dir.clone());
        // For multi-file torrents, files are laid out under <output>/<name>/.
        // Single-file torrents store the file directly under <output>/.
        let root_dir = if info.single_file_mode {
            root_dir
        } else {
            root_dir.join(&info.name)
        };

        let lengths = Lengths::new(info.total_length(), info.piece_length)
            .map_err(|e| format!("bad lengths: {e}"))?;
        let mut meta = meta;
        if let Some(extra) = opts.trackers {
            meta.announce_list.push(extra);
        }
        // Reserve an id and claim the info-hash atomically so a concurrent
        // add_from_meta cannot race past the duplicate check above. If spawn
        // fails we roll the reservation back.
        let id = {
            let mut inner = self.inner.lock();
            if let Some(&existing) = inner.by_hash.get(&meta.info_hash) {
                if let Some(t) = inner.torrents.get(&existing).cloned() {
                    return Ok(AddTorrentResponse::AlreadyManaged(existing, t));
                }
            }
            let id = inner.next_id;
            inner.next_id += 1;
            inner.by_hash.insert(meta.info_hash, id);
            id
        };
        let init = TorrentInit {
            meta: meta.clone(),
            lengths,
            root_dir,
            only_files: opts.only_files,
            max_outstanding_per_peer: self.opts.max_outstanding_requests_per_peer,
            max_peers: self.opts.max_peers_per_torrent,
            encryption: self.opts.encryption,
        };
        let handle = match spawn_torrent(id, init, self.peer_id, self.listen_port).await {
            Ok(h) => h,
            Err(e) => {
                // Roll back the reservation so a retry can succeed.
                let mut inner = self.inner.lock();
                inner.by_hash.remove(&meta.info_hash);
                return Err(format!("spawn torrent: {e}"));
            }
        };
        {
            let mut inner = self.inner.lock();
            inner.torrents.insert(id, handle.clone());
        }
        if let Some(lsd) = self.lsd.lock().as_ref() {
            lsd.add_infohash(meta.info_hash);
        }
        Ok(AddTorrentResponse::Added(id, handle))
    }

    pub fn with_torrents<F, T>(&self, f: F) -> T
    where
        F: FnOnce(&mut dyn Iterator<Item = (usize, Arc<ManagedTorrent>)>) -> T,
    {
        let snapshot: Vec<(usize, Arc<ManagedTorrent>)> = self
            .inner
            .lock()
            .torrents
            .iter()
            .map(|(id, h)| (*id, h.clone()))
            .collect();
        let mut iter = snapshot.into_iter();
        f(&mut iter)
    }

    pub fn get(&self, which: TorrentIdOrHash) -> Option<Arc<ManagedTorrent>> {
        let inner = self.inner.lock();
        match which {
            TorrentIdOrHash::Id(id) => inner.torrents.get(&id).cloned(),
            TorrentIdOrHash::Hash(h) => inner
                .by_hash
                .get(&h)
                .and_then(|id| inner.torrents.get(id))
                .cloned(),
        }
    }

    pub async fn pause(&self, handle: &Arc<ManagedTorrent>) -> Result<(), String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        handle
            .cmd_tx()
            .send(TorrentCommand::Pause(tx))
            .await
            .map_err(|e| e.to_string())?;
        rx.await.map_err(|e| e.to_string())
    }

    pub async fn unpause(&self, handle: &Arc<ManagedTorrent>) -> Result<(), String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        handle
            .cmd_tx()
            .send(TorrentCommand::Unpause(tx))
            .await
            .map_err(|e| e.to_string())?;
        rx.await.map_err(|e| e.to_string())
    }

    pub async fn delete(&self, which: TorrentIdOrHash, with_files: bool) -> Result<(), String> {
        let handle = self.get(which).ok_or_else(|| "not found".to_string())?;
        // Capture file paths for optional deletion before stopping the torrent
        // (the torrent loop owns storage and will drop it on Stop).
        let file_paths: Option<Vec<(PathBuf, bool)>> = if with_files {
            handle
                .with_metadata(|meta| {
                    let root = self.output_dir.clone();
                    if meta.info.single_file_mode {
                        // Single-file: files live directly under root
                        vec![(root.join(&meta.info.name), false)]
                    } else {
                        // Multi-file: everything under root/<name>/
                        vec![(root.join(&meta.info.name), true)]
                    }
                })
                .ok()
        } else {
            None
        };
        let (tx, rx) = tokio::sync::oneshot::channel();
        handle
            .cmd_tx()
            .send(TorrentCommand::Stop(tx))
            .await
            .map_err(|e| e.to_string())?;
        let _ = rx.await;
        {
            let mut inner = self.inner.lock();
            inner.torrents.remove(&handle.id);
            inner.by_hash.remove(&handle.info_hash);
        }
        if let Some(lsd) = self.lsd.lock().as_ref() {
            lsd.remove_infohash(handle.info_hash);
        }
        if let Some(paths) = file_paths {
            for (p, is_dir) in paths {
                let res = if is_dir {
                    tokio::fs::remove_dir_all(&p).await
                } else {
                    tokio::fs::remove_file(&p).await
                };
                match res {
                    Ok(()) => log::info!("deleted torrent data: {}", p.display()),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => log::warn!("failed to delete {}: {}", p.display(), e),
                }
            }
        }
        Ok(())
    }

    pub async fn add_peer(&self, info_hash: Id20, addr: SocketAddr) -> Result<(), String> {
        let handle = self
            .get(TorrentIdOrHash::Hash(info_hash))
            .ok_or_else(|| "torrent not found".to_string())?;
        handle
            .cmd_tx()
            .send(TorrentCommand::AddPeer(addr))
            .await
            .map_err(|e| e.to_string())
    }
}

/// Bind a TCP listener on an IPv6 address with `IPV6_V6ONLY` set to avoid
/// dual-stack conflicts when an IPv4 listener already bound the same port.
fn bind_v6_listener(addr: SocketAddr) -> std::io::Result<std::net::TcpListener> {
    use socket2::{Domain, Protocol, Socket, Type};
    let sock = Socket::new(Domain::IPV6, Type::STREAM, Some(Protocol::TCP))?;
    sock.set_only_v6(true)?;
    sock.set_reuse_address(true)?;
    sock.set_nonblocking(true)?;
    sock.bind(&addr.into())?;
    sock.listen(128)?;
    Ok(sock.into())
}

/// Shared inbound accept loop. Parameterised on a `Weak<Session>` so the
/// task does not keep the session alive; the session's Drop aborts it.
async fn run_accept_loop(listener: TcpListener, weak: std::sync::Weak<Session>) {
    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                let Some(s) = weak.upgrade() else {
                    return;
                };
                tokio::spawn(async move {
                    let allowed: Vec<Id20> = s.inner.lock().by_hash.keys().copied().collect();
                    let policy = s.opts.encryption;
                    let res = super::peer::accept_with_policy(
                        stream,
                        s.peer_id,
                        allowed,
                        std::time::Duration::from_secs(30),
                        policy,
                    )
                    .await;
                    match res {
                        Ok((handle, rx)) => {
                            s.route_inbound_peer(addr, handle.tx, rx).await;
                        }
                        Err(e) => {
                            log::debug!("inbound peer handshake failed: {e}")
                        }
                    }
                });
            }
            Err(e) => {
                log::debug!("accept failed: {e}");
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
        }
    }
}
