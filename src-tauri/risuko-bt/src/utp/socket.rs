//! The shared µTP endpoint: one UDP socket multiplexing many connections; a background router task reads every datagram and dispatches it to the right connection driver, keyed by `(peer_addr, our_recv_conn_id)`, and a SYN for an unknown key opens a new inbound connection and enqueues its [`UtpStream`] for [`UtpSocket::accept`]

use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use parking_lot::{Mutex, RwLock};
use rand::RngExt;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, oneshot, Mutex as AsyncMutex};

use risuko_http::{NoProxy, ProxyConnector, ProxyDatagram, ProxyDatagramSource};

use super::packet::{PacketType, UtpHeader};
use super::stream::{self, DatagramTransport, DriverConfig, Role, RoleKind, UtpStream};

pub(crate) type ConnKey = (SocketAddr, u16);

pub(crate) type ConnRegistry =
    Arc<Mutex<HashMap<ConnKey, mpsc::UnboundedSender<(UtpHeader, Bytes)>>>>;
pub(crate) type ProxyConnRegistry = Arc<Mutex<HashMap<u16, ConnKey>>>;

const MAX_DATAGRAM: usize = 2048;
const ROUTER_READ_SLAB: usize = MAX_DATAGRAM * 64;
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

pub struct UtpSocket {
    udp: Arc<UdpSocket>,
    registry: ConnRegistry,
    proxy_registry: ProxyConnRegistry,
    local_addr: SocketAddr,
    accept_rx: tokio::sync::Mutex<mpsc::UnboundedReceiver<UtpStream>>,
    router_handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
    proxy_router_handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
    outbound: RwLock<OutboundRoute>,
    reconfigure_lock: AsyncMutex<()>,
}

#[derive(Clone)]
enum OutboundRoute {
    Direct,
    Proxy(Arc<ProxyDatagram>),
    Blocked { error: String, bypass: NoProxy },
}

impl UtpSocket {
    /// Bind a fresh UDP socket and start serving µTP on it
    pub async fn bind(addr: SocketAddr) -> io::Result<Arc<Self>> {
        let udp = UdpSocket::bind(addr).await?;
        Ok(Self::from_udp(Arc::new(udp)))
    }

    pub async fn bind_with_proxy(
        addr: SocketAddr,
        proxy: Option<ProxyConnector>,
    ) -> io::Result<Arc<Self>> {
        let udp = Arc::new(UdpSocket::bind(addr).await?);
        let socket = Self::from_udp(udp);
        socket.reconfigure_proxy(proxy).await;
        Ok(socket)
    }

    /// Build a µTP endpoint over an existing UDP socket (e.g. one shared with another protocol on the same port)
    pub fn from_udp(udp: Arc<UdpSocket>) -> Arc<Self> {
        let local_addr = udp
            .local_addr()
            .unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], 0)));
        let registry: ConnRegistry = Arc::new(Mutex::new(HashMap::new()));
        let proxy_registry: ProxyConnRegistry = Arc::new(Mutex::new(HashMap::new()));
        let (accept_tx, accept_rx) = mpsc::unbounded_channel();
        let router_handle = tokio::spawn(router(udp.clone(), registry.clone(), accept_tx));
        Arc::new(Self {
            udp,
            registry,
            proxy_registry,
            local_addr,
            accept_rx: tokio::sync::Mutex::new(accept_rx),
            router_handle: Mutex::new(Some(router_handle)),
            proxy_router_handle: Mutex::new(None),
            outbound: RwLock::new(OutboundRoute::Direct),
            reconfigure_lock: AsyncMutex::new(()),
        })
    }

    /// Replace the outbound route while preserving the local listener
    pub async fn reconfigure_proxy(&self, proxy: Option<ProxyConnector>) {
        let route = match proxy {
            None => OutboundRoute::Direct,
            Some(connector) if connector.udp_proxy().is_none() => OutboundRoute::Direct,
            Some(connector) => {
                let bypass = connector.udp_no_proxy().unwrap_or_default();
                let bind = if bypass.is_empty() {
                    connector.bind_udp().await
                } else {
                    connector.bind_udp_with_bypass().await
                };
                match bind {
                    Ok(datagram) => OutboundRoute::Proxy(Arc::new(datagram)),
                    Err(error) => OutboundRoute::Blocked {
                        error: error.to_string(),
                        bypass,
                    },
                }
            }
        };

        let _reconfigure_guard = self.reconfigure_lock.lock().await;

        // Drop every connection routed through the old proxy before replacing
        // the router. Direct-route connections remain registered.
        {
            let mut proxy_registry = self.proxy_registry.lock();
            let old_keys = proxy_registry.values().copied().collect::<Vec<_>>();
            let mut registry = self.registry.lock();
            for key in old_keys {
                registry.remove(&key);
            }
            proxy_registry.clear();
        }
        if let Some(handle) = self.proxy_router_handle.lock().take() {
            handle.abort();
        }

        if let OutboundRoute::Proxy(datagram) = &route {
            let router_datagram = datagram.clone();
            let registry = self.registry.clone();
            let proxy_registry = self.proxy_registry.clone();
            let handle = tokio::spawn(async move {
                proxy_router(router_datagram, registry, proxy_registry).await
            });
            *self.proxy_router_handle.lock() = Some(handle);
        }
        *self.outbound.write() = route;
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Dial a peer over µTP, resolving once the handshake completes
    pub async fn connect(&self, remote: SocketAddr) -> io::Result<UtpStream> {
        self.connect_timeout(remote, DEFAULT_CONNECT_TIMEOUT).await
    }

    pub async fn connect_timeout(
        &self,
        remote: SocketAddr,
        timeout: Duration,
    ) -> io::Result<UtpStream> {
        let reconfigure_guard = self.reconfigure_lock.lock().await;
        let route = self.outbound.read().clone();
        let uses_proxy = matches!(route, OutboundRoute::Proxy(_));

        let (key, inc_rx) = {
            let mut proxy_registry = self.proxy_registry.lock();
            let mut reg = self.registry.lock();
            let mut id: u16 = rand::rng().random();
            let mut tries = 0;
            while reg.contains_key(&(remote, id))
                || (uses_proxy && proxy_registry.contains_key(&id))
            {
                id = id.wrapping_add(1);
                tries += 1;
                if tries > 64 {
                    return Err(io::Error::new(
                        io::ErrorKind::AddrInUse,
                        "no free utp connection id for peer",
                    ));
                }
            }
            let key = (remote, id);
            let (inc_tx, inc_rx) = mpsc::unbounded_channel();
            reg.insert(key, inc_tx);
            if uses_proxy {
                proxy_registry.insert(id, key);
            }
            (key, inc_rx)
        };
        let recv_id = key.1;
        let send_id = recv_id.wrapping_add(1);

        let transport = match route {
            OutboundRoute::Direct => DatagramTransport::Direct(self.udp.clone()),
            OutboundRoute::Proxy(proxy) => DatagramTransport::Proxy(proxy),
            OutboundRoute::Blocked { error, bypass } => {
                if bypass.matches_host_port(&remote.ip().to_string(), Some(remote.port())) {
                    DatagramTransport::Direct(self.udp.clone())
                } else {
                    self.registry.lock().remove(&key);
                    self.proxy_registry.lock().remove(&recv_id);
                    return Err(io::Error::new(io::ErrorKind::Unsupported, error));
                }
            }
        };

        let (done_tx, done_rx) = oneshot::channel();
        let shared = stream::new_shared(remote, send_id, RoleKind::Initiator);
        let cfg = DriverConfig {
            transport,
            remote,
            incoming: inc_rx,
            registry: self.registry.clone(),
            key,
            proxy_registry: uses_proxy.then(|| self.proxy_registry.clone()),
        };
        let driver_shared = shared.clone();
        tokio::spawn(stream::drive(driver_shared, cfg, Role::Initiator(done_tx)));
        drop(reconfigure_guard);

        match tokio::time::timeout(timeout, done_rx).await {
            Ok(Ok(Ok(()))) => Ok(UtpStream::new(shared)),
            Ok(Ok(Err(e))) => Err(e),
            Ok(Err(_)) => Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "utp driver exited before handshake",
            )),
            Err(_) => {
                // Timed out waiting for the peer's STATE; tell the driver to stop retransmitting and reclaim the slot
                {
                    let mut st = shared.state.lock();
                    st.force_close();
                }
                shared.nudge.notify_one();
                self.registry.lock().remove(&key);
                self.proxy_registry.lock().remove(&recv_id);
                Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "utp connect timed out",
                ))
            }
        }
    }

    /// Accept the next inbound µTP connection
    pub async fn accept(&self) -> io::Result<UtpStream> {
        let mut rx = self.accept_rx.lock().await;
        rx.recv()
            .await
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "utp socket closed"))
    }

    pub fn shutdown(&self) {
        if let Some(handle) = self.router_handle.lock().take() {
            handle.abort();
        }
        if let Some(handle) = self.proxy_router_handle.lock().take() {
            handle.abort();
        }
        self.registry.lock().clear();
        self.proxy_registry.lock().clear();
    }
}

impl Drop for UtpSocket {
    fn drop(&mut self) {
        if let Some(handle) = self.router_handle.get_mut().take() {
            handle.abort();
        }
        if let Some(handle) = self.proxy_router_handle.get_mut().take() {
            handle.abort();
        }
        self.registry.lock().clear();
        self.proxy_registry.lock().clear();
    }
}

/// Reads every datagram and routes it to the owning connection, or opens a new inbound connection for an unrecognized SYN
async fn router(
    udp: Arc<UdpSocket>,
    registry: ConnRegistry,
    accept_tx: mpsc::UnboundedSender<UtpStream>,
) {
    let mut buf = BytesMut::with_capacity(ROUTER_READ_SLAB);
    buf.resize(ROUTER_READ_SLAB, 0);
    loop {
        if buf.len() < MAX_DATAGRAM {
            buf.resize(ROUTER_READ_SLAB, 0);
        }
        let (n, src) = match udp.recv_from(&mut buf[..MAX_DATAGRAM]).await {
            Ok(x) => x,
            // A transient recv error (e.g. ICMP port-unreachable surfaced on some platforms) shouldn't kill the whole endpoint
            Err(_) => continue,
        };
        let Ok((header, payload)) = UtpHeader::decode(&buf[..n]) else {
            continue;
        };
        let payload_offset = n - payload.len();
        let payload = buf.split_to(n).freeze().slice(payload_offset..);
        let key = (src, header.connection_id);
        // Fast path: an established connection owns this id
        {
            let reg = registry.lock();
            if let Some(tx) = reg.get(&key) {
                let _ = tx.send((header, payload));
                continue;
            }
        }
        // Otherwise only a SYN is meaningful; everything else is a stray packet for a connection we don't have (ignored)
        if header.packet_type == PacketType::Syn {
            open_inbound(&udp, &registry, &accept_tx, src, &header);
        }
    }
}

/// Create the responder side of a connection from an inbound SYN
fn open_inbound(
    udp: &Arc<UdpSocket>,
    registry: &ConnRegistry,
    accept_tx: &mpsc::UnboundedSender<UtpStream>,
    src: SocketAddr,
    syn: &UtpHeader,
) {
    // Responder sends stamped with the SYN's id (C) and receives stamped C+1
    let send_id = syn.connection_id;
    let recv_id = send_id.wrapping_add(1);
    let key = (src, recv_id);

    let inc_rx = {
        let mut reg = registry.lock();
        if reg.contains_key(&key) {
            return; // duplicate / retransmitted SYN for an open connection
        }
        let (inc_tx, inc_rx) = mpsc::unbounded_channel();
        reg.insert(key, inc_tx);
        inc_rx
    };

    let shared = stream::new_shared(src, send_id, RoleKind::Responder);
    shared.state.lock().seed_responder(syn);

    let cfg = DriverConfig {
        transport: DatagramTransport::Direct(udp.clone()),
        remote: src,
        incoming: inc_rx,
        registry: registry.clone(),
        key,
        proxy_registry: None,
    };
    tokio::spawn(stream::drive(shared.clone(), cfg, Role::Responder));
    // If nobody is accepting, the stream drops immediately and its driver tears the connection down cleanly
    let _ = accept_tx.send(UtpStream::new(shared));
}

async fn proxy_router(
    datagram: Arc<ProxyDatagram>,
    registry: ConnRegistry,
    proxy_registry: ProxyConnRegistry,
) {
    let mut buf = BytesMut::with_capacity(ROUTER_READ_SLAB);
    buf.resize(ROUTER_READ_SLAB, 0);
    loop {
        if buf.len() < MAX_DATAGRAM {
            buf.resize(ROUTER_READ_SLAB, 0);
        }
        let (n, src) = match datagram.recv_from_target(&mut buf[..MAX_DATAGRAM]).await {
            Ok(value) => value,
            Err(error) => {
                tracing::warn!("uTP proxy receive loop terminated: {error}");
                return;
            }
        };
        let Ok((header, payload)) = UtpHeader::decode(&buf[..n]) else {
            continue;
        };
        let payload_offset = n - payload.len();
        let payload = buf.split_to(n).freeze().slice(payload_offset..);
        match src {
            ProxyDatagramSource::Ip(src) => {
                let key = (src, header.connection_id);
                if let Some(tx) = registry.lock().get(&key) {
                    let _ = tx.send((header, payload));
                }
            }
            ProxyDatagramSource::Host(host, port) => {
                let key = proxy_registry.lock().get(&header.connection_id).copied();
                let Some(key) = key else {
                    continue;
                };
                if key.0.port() != port
                    || !risuko_http::datagram_source_matches(
                        &ProxyDatagramSource::Host(host, port),
                        key.0,
                    )
                {
                    continue;
                }
                if let Some(tx) = registry.lock().get(&key) {
                    let _ = tx.send((header, payload));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn loopback_pair() -> (Arc<UtpSocket>, Arc<UtpSocket>) {
        let a = UtpSocket::bind("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let b = UtpSocket::bind("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        (a, b)
    }

    #[tokio::test]
    async fn handshake_then_echo() {
        let (client_sock, server_sock) = loopback_pair().await;
        let server_addr = server_sock.local_addr();

        let server = tokio::spawn(async move {
            let mut s = server_sock.accept().await.unwrap();
            let mut buf = [0u8; 5];
            s.read_exact(&mut buf).await.unwrap();
            s.write_all(&buf).await.unwrap();
            s.flush().await.unwrap();
            // Hold the connection open until the client reads the echo
            tokio::time::sleep(Duration::from_millis(300)).await;
        });

        tokio::time::timeout(Duration::from_secs(5), async move {
            let mut c = client_sock.connect(server_addr).await.unwrap();
            c.write_all(b"hello").await.unwrap();
            c.flush().await.unwrap();
            let mut echo = [0u8; 5];
            c.read_exact(&mut echo).await.unwrap();
            assert_eq!(&echo, b"hello");
        })
        .await
        .expect("echo round trip timed out");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn bulk_transfer_preserves_bytes() {
        let (client_sock, server_sock) = loopback_pair().await;
        let server_addr = server_sock.local_addr();
        // Many MSS-sized packets to exercise sequencing, acks, and the window
        const N: usize = 256 * 1024;
        let data: Vec<u8> = (0..N).map(|i| (i % 251) as u8).collect();
        let expected = data.clone();

        let server = tokio::spawn(async move {
            let mut s = server_sock.accept().await.unwrap();
            let mut got = Vec::new();
            s.read_to_end(&mut got).await.unwrap();
            got
        });

        tokio::time::timeout(Duration::from_secs(20), async move {
            let mut c = client_sock.connect(server_addr).await.unwrap();
            c.write_all(&data).await.unwrap();
            // Clean FIN; the server's read_to_end completes on the resulting EOF
            c.shutdown().await.unwrap();
        })
        .await
        .expect("bulk send timed out");

        let got = tokio::time::timeout(Duration::from_secs(20), server)
            .await
            .expect("bulk recv timed out")
            .unwrap();
        assert_eq!(got.len(), expected.len());
        assert_eq!(got, expected);
    }

    #[tokio::test]
    async fn connect_to_dead_peer_times_out() {
        let sock = UtpSocket::bind("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        // 127.0.0.1:1 has no µTP listener; the SYN goes unanswered
        let dead: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let err = sock
            .connect_timeout(dead, Duration::from_millis(600))
            .await
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
    }

    #[tokio::test]
    async fn drop_clears_registry_to_close_driver_channels() {
        let udp = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let sock = UtpSocket::from_udp(udp);
        let registry = sock.registry.clone();
        let key: ConnKey = ("127.0.0.1:9".parse().unwrap(), 7);
        let (tx, mut rx) = mpsc::unbounded_channel();
        registry.lock().insert(key, tx);

        drop(sock);

        assert!(registry.lock().is_empty());
        let received = tokio::time::timeout(Duration::from_millis(100), rx.recv())
            .await
            .expect("registry sender kept the driver channel open");
        assert!(received.is_none());
    }
}
