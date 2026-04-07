use std::net::SocketAddrV4;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

use super::protocol::*;
use super::types::*;

/// Events from a peer connection
#[derive(Debug)]
pub enum PeerEvent {
    HelloAnswer,
    FileStatus { file_hash: [u8; 16], parts: Vec<bool> },
    HashsetAnswer { file_hash: [u8; 16], hashes: Vec<[u8; 16]> },
    SlotGiven,
    SlotTaken,
    QueueRanking(u16),
    DataReceived { start: u32, data: Vec<u8> },
    Disconnected(Option<String>),
}

/// A connection to a peer for file transfer
pub struct PeerConnection {
    addr: SocketAddrV4,
    client_hash: [u8; 16],
    client_id: u32,
    client_port: u16,
    server_ip: u32,
    server_port: u16,
    tx: Option<mpsc::Sender<Ed2kPacket>>,
}

impl PeerConnection {
    pub fn new(
        addr: SocketAddrV4,
        client_hash: [u8; 16],
        client_id: u32,
        client_port: u16,
        server_ip: u32,
        server_port: u16,
    ) -> Self {
        Self {
            addr,
            client_hash,
            client_id,
            client_port,
            server_ip,
            server_port,
            tx: None,
        }
    }

    /// Connect to peer and run the communication loop
    pub async fn connect(
        &mut self,
    ) -> Result<(mpsc::Receiver<PeerEvent>, mpsc::Sender<Ed2kPacket>), String> {
        let stream = TcpStream::connect(self.addr)
            .await
            .map_err(|e| format!("Failed to connect to peer {}: {}", self.addr, e))?;

        let (read_half, mut write_half) = stream.into_split();

        // Send hello
        let hello = build_hello_client(
            &self.client_hash,
            self.client_id,
            self.client_port,
            self.server_ip,
            self.server_port,
        );
        write_half
            .write_all(&hello.encode())
            .await
            .map_err(|e| format!("Failed to send hello to peer: {}", e))?;

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
            let mut buf = bytes::BytesMut::with_capacity(65536);
            loop {
                match reader.read_buf(&mut buf).await {
                    Ok(0) => {
                        let _ = event_tx_clone.send(PeerEvent::Disconnected(None)).await;
                        break;
                    }
                    Ok(_) => {
                        while let Ok(Some(packet)) = Ed2kPacket::decode(&mut buf) {
                            if Self::handle_peer_packet(&event_tx_clone, &packet)
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        let _ = event_tx_clone
                            .send(PeerEvent::Disconnected(Some(e.to_string())))
                            .await;
                        break;
                    }
                }
            }
        });

        Ok((event_rx, packet_tx))
    }

    async fn handle_peer_packet(
        tx: &mpsc::Sender<PeerEvent>,
        packet: &Ed2kPacket,
    ) -> Result<(), ()> {
        let event = match packet.opcode {
            OP_HELLO_ANSWER => {
                if packet.payload.len() < 17 {
                    return Ok(());
                }
                let mut hash = [0u8; 16];
                // Skip first byte (hash length = 0x10)
                hash.copy_from_slice(&packet.payload[1..17]);
                PeerEvent::HelloAnswer
            }
            OP_FILE_STATUS => {
                let (hash, parts) = parse_file_status(&packet.payload).map_err(|_| ())?;
                PeerEvent::FileStatus {
                    file_hash: hash,
                    parts,
                }
            }
            OP_HASHSET_ANSWER => {
                let (hash, hashes) = parse_hashset_answer(&packet.payload).map_err(|_| ())?;
                PeerEvent::HashsetAnswer {
                    file_hash: hash,
                    hashes,
                }
            }
            OP_SLOT_GIVEN => PeerEvent::SlotGiven,
            OP_SLOT_TAKEN => PeerEvent::SlotTaken,
            OP_EMULE_QUEUE_RANKING => {
                let rank = if packet.payload.len() >= 2 {
                    u16::from_le_bytes([packet.payload[0], packet.payload[1]])
                } else {
                    0
                };
                PeerEvent::QueueRanking(rank)
            }
            OP_SENDING_PART => {
                let (_hash, start, _end) =
                    parse_sending_part_header(&packet.payload).map_err(|_| ())?;
                let data_offset = 24; // 16 (hash) + 4 (start) + 4 (end)
                let data = packet.payload[data_offset..].to_vec();
                PeerEvent::DataReceived { start, data }
            }
            _ => return Ok(()),
        };

        tx.send(event).await.map_err(|_| ())
    }

    /// Request file info
    pub async fn request_file(&self, file_hash: &[u8; 16]) -> Result<(), String> {
        let tx = self.tx.as_ref().ok_or("Not connected")?;
        tx.send(build_file_request(file_hash))
            .await
            .map_err(|_| "Send failed".to_string())
    }

    /// Request file status (part bitmap)
    pub async fn request_file_status(&self, file_hash: &[u8; 16]) -> Result<(), String> {
        let tx = self.tx.as_ref().ok_or("Not connected")?;
        tx.send(build_file_status_request(file_hash))
            .await
            .map_err(|_| "Send failed".to_string())
    }

    /// Request chunk hashes
    pub async fn request_hashset(&self, file_hash: &[u8; 16]) -> Result<(), String> {
        let tx = self.tx.as_ref().ok_or("Not connected")?;
        tx.send(build_hashset_request(file_hash))
            .await
            .map_err(|_| "Send failed".to_string())
    }

    /// Request a download slot
    pub async fn request_slot(&self, file_hash: &[u8; 16]) -> Result<(), String> {
        let tx = self.tx.as_ref().ok_or("Not connected")?;
        tx.send(build_slot_request(file_hash))
            .await
            .map_err(|_| "Send failed".to_string())
    }

    /// Request data parts (up to 3 ranges)
    pub async fn request_parts(
        &self,
        file_hash: &[u8; 16],
        ranges: &[(u32, u32)],
    ) -> Result<(), String> {
        let tx = self.tx.as_ref().ok_or("Not connected")?;
        tx.send(build_request_parts(file_hash, ranges))
            .await
            .map_err(|_| "Send failed".to_string())
    }
}
