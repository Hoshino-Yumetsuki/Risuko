use std::net::SocketAddrV4;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

use super::protocol::*;
use super::types::*;

/// Events emitted by the server connection
#[derive(Debug)]
pub enum ServerEvent {
    Connected {
        client_id: u32,
    },
    ServerMessage(String),
    ServerStatus {
        users: u32,
        files: u32,
    },
    FoundSources {
        file_hash: [u8; 16],
        sources: Vec<(u32, u16)>,
    },
    ServerList,
    Disconnected(Option<String>),
}

/// A TCP connection to an ed2k server
pub struct ServerConnection {
    addr: SocketAddrV4,
    client_hash: [u8; 16],
    client_port: u16,
    _client_id: u32,
    tx: Option<mpsc::Sender<Ed2kPacket>>,
}

impl ServerConnection {
    pub fn new(addr: SocketAddrV4, client_hash: [u8; 16], client_port: u16) -> Self {
        Self {
            addr,
            client_hash,
            client_port,
            _client_id: 0,
            tx: None,
        }
    }

    /// Connect and run the server communication loop
    /// Returns a channel to receive events and a sender to queue outgoing packets
    pub async fn connect(
        &mut self,
    ) -> Result<(mpsc::Receiver<ServerEvent>, mpsc::Sender<Ed2kPacket>), String> {
        let stream = TcpStream::connect(self.addr)
            .await
            .map_err(|e| format!("Failed to connect to {}: {}", self.addr, e))?;

        let (read_half, mut write_half) = stream.into_split();

        // Send hello
        let hello = build_hello_server(&self.client_hash, self.client_port);
        write_half
            .write_all(&hello.encode())
            .await
            .map_err(|e| format!("Failed to send hello: {}", e))?;

        // Offer empty file list
        let offer = build_offer_files_empty();
        write_half
            .write_all(&offer.encode())
            .await
            .map_err(|e| format!("Failed to send offer: {}", e))?;

        let (event_tx, event_rx) = mpsc::channel(64);
        let (packet_tx, mut packet_rx) = mpsc::channel::<Ed2kPacket>(32);
        self.tx = Some(packet_tx.clone());

        // Writer task
        tokio::spawn(async move {
            while let Some(packet) = packet_rx.recv().await {
                if write_half.write_all(&packet.encode()).await.is_err() {
                    break;
                }
            }
        });

        // Reader task
        let event_tx_clone = event_tx.clone();
        tokio::spawn(async move {
            let mut reader = read_half;
            let mut buf = bytes::BytesMut::with_capacity(8192);
            loop {
                match reader.read_buf(&mut buf).await {
                    Ok(0) => {
                        let _ = event_tx_clone.send(ServerEvent::Disconnected(None)).await;
                        break;
                    }
                    Ok(_) => {
                        while let Ok(Some(packet)) = Ed2kPacket::decode(&mut buf) {
                            if let Err(_) =
                                Self::handle_server_packet(&event_tx_clone, &packet).await
                            {
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        let _ = event_tx_clone
                            .send(ServerEvent::Disconnected(Some(e.to_string())))
                            .await;
                        break;
                    }
                }
            }
        });

        Ok((event_rx, packet_tx))
    }

    async fn handle_server_packet(
        tx: &mpsc::Sender<ServerEvent>,
        packet: &Ed2kPacket,
    ) -> Result<(), ()> {
        let event = match packet.opcode {
            OP_ID_CHANGE => {
                let client_id = parse_id_change(&packet.payload).map_err(|_| ())?;
                ServerEvent::Connected { client_id }
            }
            OP_SERVER_MESSAGE => {
                let msg = parse_server_message(&packet.payload).unwrap_or_default();
                ServerEvent::ServerMessage(msg)
            }
            OP_SERVER_STATUS => {
                let (users, files) = parse_server_status(&packet.payload).map_err(|_| ())?;
                ServerEvent::ServerStatus { users, files }
            }
            OP_FOUND_SOURCES => {
                let (hash, sources) = parse_found_sources(&packet.payload).map_err(|_| ())?;
                ServerEvent::FoundSources {
                    file_hash: hash,
                    sources,
                }
            }
            OP_SERVER_LIST => ServerEvent::ServerList,
            _ => return Ok(()), // Ignore unknown opcodes
        };

        tx.send(event).await.map_err(|_| ())
    }

    /// Send a GetSources request for a file
    pub async fn request_sources(&self, file_hash: &[u8; 16]) -> Result<(), String> {
        let tx = self.tx.as_ref().ok_or("Not connected")?;
        let packet = build_get_sources(file_hash);
        tx.send(packet)
            .await
            .map_err(|_| "Send channel closed".to_string())
    }
}
