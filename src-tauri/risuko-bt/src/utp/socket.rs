//! The shared µTP endpoint: one UDP socket multiplexing many connections.
//!
//! A background router task reads every datagram and dispatches it to the
//! right connection driver, keyed by `(peer_addr, our_recv_conn_id)`. A SYN
//! for an unknown key opens a new inbound connection and enqueues its
//! [`UtpStream`] for [`UtpSocket::accept`].

use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use rand::RngExt;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, oneshot};

use super::packet::{PacketType, UtpHeader};
use super::stream::{self, DriverConfig, Role, RoleKind, UtpStream};

/// Demux key: a connection is identified by (peer address, our receive id).
pub(crate) type ConnKey = (SocketAddr, u16);

/// Maps each live connection to the channel its driver reads packets from.
pub(crate) type ConnRegistry =
    Arc<Mutex<HashMap<ConnKey, mpsc::UnboundedSender<(UtpHeader, Vec<u8>)>>>>;

/// Largest UDP datagram we'll read (µTP payloads are MSS-sized; this leaves
/// room for the header plus any extensions).
const MAX_DATAGRAM: usize = 2048;
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// A µTP endpoint sharing a single UDP socket across all its connections.
pub struct UtpSocket {
    udp: Arc<UdpSocket>,
    registry: ConnRegistry,
    local_addr: SocketAddr,
    accept_rx: tokio::sync::Mutex<mpsc::UnboundedReceiver<UtpStream>>,
}

impl UtpSocket {
    /// Bind a fresh UDP socket and start serving µTP on it.
    pub async fn bind(addr: SocketAddr) -> io::Result<Arc<Self>> {
        let udp = UdpSocket::bind(addr).await?;
        Ok(Self::from_udp(Arc::new(udp)))
    }

    /// Build a µTP endpoint over an existing UDP socket (e.g. one shared with
    /// another protocol on the same port).
    pub fn from_udp(udp: Arc<UdpSocket>) -> Arc<Self> {
        let local_addr = udp
            .local_addr()
            .unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], 0)));
        let registry: ConnRegistry = Arc::new(Mutex::new(HashMap::new()));
        let (accept_tx, accept_rx) = mpsc::unbounded_channel();
        tokio::spawn(router(udp.clone(), registry.clone(), accept_tx));
        Arc::new(Self {
            udp,
            registry,
            local_addr,
            accept_rx: tokio::sync::Mutex::new(accept_rx),
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Dial a peer over µTP, resolving once the handshake completes.
    pub async fn connect(&self, remote: SocketAddr) -> io::Result<UtpStream> {
        self.connect_timeout(remote, DEFAULT_CONNECT_TIMEOUT).await
    }

    pub async fn connect_timeout(
        &self,
        remote: SocketAddr,
        timeout: Duration,
    ) -> io::Result<UtpStream> {
        // Reserve a receive id that doesn't collide with an existing
        // connection to this peer (and register the incoming channel).
        let (key, inc_rx) = {
            let mut reg = self.registry.lock();
            let mut id: u16 = rand::rng().random();
            let mut tries = 0;
            while reg.contains_key(&(remote, id)) {
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
            (key, inc_rx)
        };
        let recv_id = key.1;
        let send_id = recv_id.wrapping_add(1);

        let (done_tx, done_rx) = oneshot::channel();
        let shared = stream::new_shared(remote, send_id, RoleKind::Initiator);
        let cfg = DriverConfig {
            udp: self.udp.clone(),
            remote,
            incoming: inc_rx,
            registry: self.registry.clone(),
            key,
        };
        let driver_shared = shared.clone();
        tokio::spawn(stream::drive(driver_shared, cfg, Role::Initiator(done_tx)));

        match tokio::time::timeout(timeout, done_rx).await {
            Ok(Ok(Ok(()))) => Ok(UtpStream::new(shared)),
            Ok(Ok(Err(e))) => Err(e),
            Ok(Err(_)) => Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "utp driver exited before handshake",
            )),
            Err(_) => {
                // Timed out waiting for the peer's STATE. Tell the driver to
                // stop retransmitting and reclaim the slot.
                {
                    let mut st = shared.state.lock();
                    st.force_close();
                }
                shared.nudge.notify_one();
                self.registry.lock().remove(&key);
                Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "utp connect timed out",
                ))
            }
        }
    }

    /// Accept the next inbound µTP connection.
    pub async fn accept(&self) -> io::Result<UtpStream> {
        let mut rx = self.accept_rx.lock().await;
        rx.recv()
            .await
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "utp socket closed"))
    }
}

/// Reads every datagram and routes it to the owning connection, or opens a new
/// inbound connection for an unrecognized SYN.
async fn router(
    udp: Arc<UdpSocket>,
    registry: ConnRegistry,
    accept_tx: mpsc::UnboundedSender<UtpStream>,
) {
    let mut buf = vec![0u8; MAX_DATAGRAM];
    loop {
        let (n, src) = match udp.recv_from(&mut buf).await {
            Ok(x) => x,
            // A transient recv error (e.g. ICMP port-unreachable surfaced on
            // some platforms) shouldn't kill the whole endpoint.
            Err(_) => continue,
        };
        let Ok((header, payload)) = UtpHeader::decode(&buf[..n]) else {
            continue;
        };
        let key = (src, header.connection_id);
        // Fast path: an established connection owns this id.
        {
            let reg = registry.lock();
            if let Some(tx) = reg.get(&key) {
                let _ = tx.send((header, payload.to_vec()));
                continue;
            }
        }
        // Otherwise only a SYN is meaningful; everything else is a stray
        // packet for a connection we don't have (ignored).
        if header.packet_type == PacketType::Syn {
            open_inbound(&udp, &registry, &accept_tx, src, &header);
        }
    }
}

/// Create the responder side of a connection from an inbound SYN.
fn open_inbound(
    udp: &Arc<UdpSocket>,
    registry: &ConnRegistry,
    accept_tx: &mpsc::UnboundedSender<UtpStream>,
    src: SocketAddr,
    syn: &UtpHeader,
) {
    // Responder sends stamped with the SYN's id (C) and receives stamped C+1.
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
        udp: udp.clone(),
        remote: src,
        incoming: inc_rx,
        registry: registry.clone(),
        key,
    };
    tokio::spawn(stream::drive(shared.clone(), cfg, Role::Responder));
    // If nobody is accepting, the stream drops immediately and its driver
    // tears the connection down cleanly.
    let _ = accept_tx.send(UtpStream::new(shared));
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
            // Hold the connection open until the client reads the echo.
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
        // Many MSS-sized packets to exercise sequencing, acks, and the window.
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
            // Clean FIN; the server's read_to_end completes on the resulting EOF.
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
        // 127.0.0.1:1 has no µTP listener; the SYN goes unanswered.
        let dead: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let err = sock
            .connect_timeout(dead, Duration::from_millis(600))
            .await
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
    }
}
