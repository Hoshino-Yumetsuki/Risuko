//! Per-peer async actor

use std::net::SocketAddr;
use std::time::Duration;

use bytes::BytesMut;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::timeout;

use super::super::core::Id20;
use super::super::wire::{Handshake, Message, MessageDecoder, MessageEncoder, HANDSHAKE_LEN};

/// Events emitted by the peer task toward the torrent
#[derive(Debug)]
pub enum PeerEvent {
    /// Handshake finished successfully
    Handshook {
        peer_id: Id20,
        reserved: [u8; 8],
        /// Info hash negotiated during the handshake. For outbound
        /// connections this matches the requested hash; for inbound ones it
        /// identifies which torrent the peer wants
        info_hash: Id20,
    },
    /// Decoded peer message
    Message(Message),
    /// Connection ended (clean EOF or error)
    Disconnected { reason: String },
}

/// Commands sent from the torrent to the peer writer task
#[derive(Debug)]
pub enum PeerCommand {
    Send(Message),
    /// Abort the connection
    Disconnect,
}

/// Opaque handle to a running peer task
pub struct PeerHandle {
    pub addr: SocketAddr,
    pub tx: mpsc::Sender<PeerCommand>,
}

/// Parameters for spawning an outbound peer connection
pub struct SpawnPeer {
    pub addr: SocketAddr,
    pub info_hash: Id20,
    pub our_peer_id: Id20,
    pub connect_timeout: Duration,
    pub read_timeout: Duration,
}

/// Connect to a peer, perform the BEP-3 handshake, and split the socket into
/// reader/writer tasks. Returns (handle, event receiver)
pub async fn connect(spawn: SpawnPeer) -> std::io::Result<(PeerHandle, mpsc::Receiver<PeerEvent>)> {
    let stream = timeout(spawn.connect_timeout, TcpStream::connect(spawn.addr))
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "connect timeout"))??;
    stream.set_nodelay(true).ok();
    drive_handshake(stream, spawn).await
}

/// Accept an inbound peer connection: peer sends handshake first, we reply
pub async fn accept(
    stream: TcpStream,
    our_peer_id: Id20,
    allowed: impl Fn(&Id20) -> bool,
    read_timeout: Duration,
) -> std::io::Result<(PeerHandle, mpsc::Receiver<PeerEvent>)> {
    stream.set_nodelay(true).ok();
    let addr = stream.peer_addr()?;
    let (mut reader, mut writer) = stream.into_split();

    let mut buf = [0u8; HANDSHAKE_LEN];
    timeout(read_timeout, reader.read_exact(&mut buf))
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "hs read timeout"))??;
    let remote_hs = Handshake::parse(&buf)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{e}")))?;
    if !allowed(&remote_hs.info_hash) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "unknown info hash",
        ));
    }
    let our_hs = Handshake::new(remote_hs.info_hash, our_peer_id);
    writer.write_all(&our_hs.to_bytes()).await?;

    finish_split(addr, remote_hs, tokio::io::BufReader::new(reader), writer)
}

async fn drive_handshake(
    stream: TcpStream,
    spawn: SpawnPeer,
) -> std::io::Result<(PeerHandle, mpsc::Receiver<PeerEvent>)> {
    let addr = spawn.addr;
    let (mut reader, mut writer) = stream.into_split();
    let our_hs = Handshake::new(spawn.info_hash, spawn.our_peer_id);
    writer.write_all(&our_hs.to_bytes()).await?;

    let mut buf = [0u8; HANDSHAKE_LEN];
    timeout(spawn.read_timeout, reader.read_exact(&mut buf))
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "hs read timeout"))??;
    let remote_hs = Handshake::parse(&buf)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{e}")))?;
    if remote_hs.info_hash != spawn.info_hash {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "info hash mismatch",
        ));
    }
    finish_split(addr, remote_hs, tokio::io::BufReader::new(reader), writer)
}

fn finish_split(
    addr: SocketAddr,
    remote_hs: Handshake,
    reader: tokio::io::BufReader<tokio::net::tcp::OwnedReadHalf>,
    writer: tokio::net::tcp::OwnedWriteHalf,
) -> std::io::Result<(PeerHandle, mpsc::Receiver<PeerEvent>)> {
    // Sized for high-throughput pipelining: with up to ~64 outstanding
    // chunk requests and Piece replies of 16 KiB arriving back-to-back,
    // a 64-slot channel would backpressure the reader and cap throughput
    let (event_tx, event_rx) = mpsc::channel(1024);
    let (cmd_tx, cmd_rx) = mpsc::channel(1024);

    // Deliver the handshake synchronously before spawning the reader so that
    // consumers always observe `Handshook` before any peer messages. The
    // channel was just created with capacity 1024 so this cannot block.
    event_tx
        .try_send(PeerEvent::Handshook {
            peer_id: remote_hs.peer_id,
            reserved: remote_hs.reserved,
            info_hash: remote_hs.info_hash,
        })
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("{e}")))?;

    tokio::spawn(reader_task(reader, event_tx));
    tokio::spawn(writer_task(writer, cmd_rx));

    Ok((PeerHandle { addr, tx: cmd_tx }, event_rx))
}

async fn reader_task(
    mut reader: tokio::io::BufReader<tokio::net::tcp::OwnedReadHalf>,
    tx: mpsc::Sender<PeerEvent>,
) {
    let mut buf = BytesMut::with_capacity(64 * 1024);
    let mut tmp = [0u8; 16 * 1024];
    loop {
        // Try to decode any complete frame already buffered
        loop {
            match MessageDecoder::try_decode(&mut buf) {
                Ok(Some(msg)) => {
                    if tx.send(PeerEvent::Message(msg)).await.is_err() {
                        return;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    let _ = tx
                        .send(PeerEvent::Disconnected {
                            reason: format!("decode: {e}"),
                        })
                        .await;
                    return;
                }
            }
        }
        match reader.read(&mut tmp).await {
            Ok(0) => {
                let _ = tx
                    .send(PeerEvent::Disconnected {
                        reason: "eof".into(),
                    })
                    .await;
                return;
            }
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(e) => {
                let _ = tx
                    .send(PeerEvent::Disconnected {
                        reason: format!("io: {e}"),
                    })
                    .await;
                return;
            }
        }
    }
}

async fn writer_task(
    mut writer: tokio::net::tcp::OwnedWriteHalf,
    mut rx: mpsc::Receiver<PeerCommand>,
) {
    while let Some(cmd) = rx.recv().await {
        match cmd {
            PeerCommand::Send(msg) => {
                let bytes = MessageEncoder::encode(&msg);
                if writer.write_all(&bytes).await.is_err() {
                    return;
                }
            }
            PeerCommand::Disconnect => {
                let _ = writer.shutdown().await;
                return;
            }
        }
    }
    let _ = writer.shutdown().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::Message;
    use tokio::net::TcpListener;

    async fn run_pair() -> (
        PeerHandle,
        mpsc::Receiver<PeerEvent>,
        PeerHandle,
        mpsc::Receiver<PeerEvent>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let info_hash = Id20([1u8; 20]);
        let peer_a = Id20([2u8; 20]);
        let peer_b = Id20([3u8; 20]);

        let accept_fut = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            accept(
                stream,
                peer_b,
                move |ih| *ih == info_hash,
                Duration::from_secs(5),
            )
            .await
            .unwrap()
        });

        let (handle_a, rx_a) = connect(SpawnPeer {
            addr,
            info_hash,
            our_peer_id: peer_a,
            connect_timeout: Duration::from_secs(5),
            read_timeout: Duration::from_secs(5),
        })
        .await
        .unwrap();

        let (handle_b, rx_b) = accept_fut.await.unwrap();
        (handle_a, rx_a, handle_b, rx_b)
    }

    #[tokio::test]
    async fn handshake_and_message() {
        let (a, mut rx_a, b, mut rx_b) = run_pair().await;

        // Both sides should see a Handshook event
        assert!(matches!(
            rx_a.recv().await.unwrap(),
            PeerEvent::Handshook { .. }
        ));
        assert!(matches!(
            rx_b.recv().await.unwrap(),
            PeerEvent::Handshook { .. }
        ));

        a.tx.send(PeerCommand::Send(Message::Interested))
            .await
            .unwrap();
        let ev = rx_b.recv().await.unwrap();
        match ev {
            PeerEvent::Message(Message::Interested) => {}
            _ => panic!("expected Interested, got {ev:?}"),
        }
        b.tx.send(PeerCommand::Send(Message::Have { piece_index: 42 }))
            .await
            .unwrap();
        let ev = rx_a.recv().await.unwrap();
        match ev {
            PeerEvent::Message(Message::Have { piece_index }) => assert_eq!(piece_index, 42),
            _ => panic!("expected Have"),
        }
    }

    #[tokio::test]
    async fn rejects_wrong_info_hash() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hash_a = Id20([1u8; 20]);
        let hash_b = Id20([9u8; 20]);
        let peer_a = Id20([2u8; 20]);
        let peer_b = Id20([3u8; 20]);

        let accept_fut = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            accept(
                stream,
                peer_b,
                move |ih| *ih == hash_b,
                Duration::from_secs(5),
            )
            .await
        });

        let res = connect(SpawnPeer {
            addr,
            info_hash: hash_a,
            our_peer_id: peer_a,
            connect_timeout: Duration::from_secs(5),
            read_timeout: Duration::from_secs(5),
        })
        .await;
        assert!(res.is_err() || accept_fut.await.unwrap().is_err());
    }
}
