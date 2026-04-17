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
use super::peer::{accept as accept_peer, PeerCommand, PeerEvent};
use super::torrent::{spawn as spawn_torrent, ManagedTorrent, TorrentCommand, TorrentInit};

#[derive(Clone, Debug)]
pub enum SessionPersistenceConfig {
    Json { folder: Option<PathBuf> },
}

#[derive(Clone, Debug, Default)]
pub struct ListenerOptions {
    pub listen_addr: Option<SocketAddr>,
    pub enable_upnp_port_forwarding: bool,
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

        if opts
            .listen
            .as_ref()
            .map(|l| l.enable_upnp_port_forwarding)
            .unwrap_or(false)
        {
            let _ = super::upnp::map_port(local_port).await;
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
        });

        let s = session.clone();
        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, addr)) => {
                        let s = s.clone();
                        tokio::spawn(async move {
                            let allowed: Vec<Id20> =
                                s.inner.lock().by_hash.keys().copied().collect();
                            let res = accept_peer(
                                stream,
                                s.peer_id,
                                move |ih| allowed.contains(ih),
                                std::time::Duration::from_secs(30),
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
        });

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

        let lengths = Lengths::new(info.total_length(), info.piece_length)
            .map_err(|e| format!("bad lengths: {e}"))?;
        let mut meta = meta;
        if let Some(extra) = opts.trackers {
            meta.announce_list.push(extra);
        }
        let id = {
            let mut inner = self.inner.lock();
            let id = inner.next_id;
            inner.next_id += 1;
            id
        };
        let init = TorrentInit {
            meta: meta.clone(),
            lengths,
            root_dir,
            only_files: opts.only_files,
            max_outstanding_per_peer: self.opts.max_outstanding_requests_per_peer,
            max_peers: self.opts.max_peers_per_torrent,
        };
        let handle = spawn_torrent(id, init, self.peer_id, self.listen_port)
            .await
            .map_err(|e| format!("spawn torrent: {e}"))?;
        {
            let mut inner = self.inner.lock();
            inner.torrents.insert(id, handle.clone());
            inner.by_hash.insert(meta.info_hash, id);
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

    pub async fn delete(&self, which: TorrentIdOrHash, _with_files: bool) -> Result<(), String> {
        let handle = self.get(which).ok_or_else(|| "not found".to_string())?;
        let (tx, rx) = tokio::sync::oneshot::channel();
        handle
            .cmd_tx()
            .send(TorrentCommand::Stop(tx))
            .await
            .map_err(|e| e.to_string())?;
        let _ = rx.await;
        let mut inner = self.inner.lock();
        inner.torrents.remove(&handle.id);
        inner.by_hash.remove(&handle.info_hash);
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
