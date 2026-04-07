#![allow(dead_code)]

use std::net::SocketAddrV4;
use tokio::net::UdpSocket;

use super::protocol::*;
use super::types::*;

/// Manages UDP communication with ed2k servers for fast source queries
pub struct UdpClient {
    socket: Option<UdpSocket>,
}

impl UdpClient {
    pub fn new() -> Self {
        Self { socket: None }
    }

    /// Bind to a local UDP port
    pub async fn bind(&mut self, port: u16) -> Result<(), String> {
        let addr = format!("0.0.0.0:{}", port);
        let socket = UdpSocket::bind(&addr)
            .await
            .map_err(|e| format!("Failed to bind UDP {}: {}", addr, e))?;
        self.socket = Some(socket);
        Ok(())
    }

    /// Send a UDP get-sources query to a server
    pub async fn query_sources(
        &self,
        server: SocketAddrV4,
        file_hash: &[u8; 16],
    ) -> Result<(), String> {
        let socket = self.socket.as_ref().ok_or("UDP not bound")?;
        let packet = build_udp_get_sources(file_hash);
        socket
            .send_to(&packet, server)
            .await
            .map_err(|e| format!("UDP send failed: {}", e))?;
        Ok(())
    }

    /// Send a UDP server status request
    pub async fn query_server_status(
        &self,
        server: SocketAddrV4,
        challenge: u32,
    ) -> Result<(), String> {
        let socket = self.socket.as_ref().ok_or("UDP not bound")?;
        let packet = build_udp_server_status_req(challenge);
        socket
            .send_to(&packet, server)
            .await
            .map_err(|e| format!("UDP send failed: {}", e))?;
        Ok(())
    }

    /// Receive and parse a single UDP response
    pub async fn recv(&self) -> Result<UdpResponse, String> {
        let socket = self.socket.as_ref().ok_or("UDP not bound")?;
        let mut buf = [0u8; 2048];
        let (len, _from) = socket
            .recv_from(&mut buf)
            .await
            .map_err(|e| format!("UDP recv failed: {}", e))?;

        if len < 2 {
            return Err("UDP packet too short".to_string());
        }

        let _proto = buf[0]; // should be PROTO_EDONKEY
        let opcode = buf[1];
        let data = &buf[2..len];

        match opcode {
            OP_UDP_FOUND_SOURCES => {
                let (hash, sources) = parse_udp_found_sources(data)?;
                Ok(UdpResponse::FoundSources { file_hash: hash, sources })
            }
            OP_UDP_SERVER_STATUS => {
                if data.len() >= 8 {
                    let challenge = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
                    let users = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
                    let files = if data.len() >= 12 {
                        u32::from_le_bytes([data[8], data[9], data[10], data[11]])
                    } else {
                        0
                    };
                    Ok(UdpResponse::ServerStatus { challenge, users, files })
                } else {
                    Err("UDP server status too short".to_string())
                }
            }
            _ => Ok(UdpResponse::Unknown(opcode)),
        }
    }
}

/// Parsed UDP response
#[derive(Debug)]
pub enum UdpResponse {
    FoundSources {
        file_hash: [u8; 16],
        sources: Vec<(u32, u16)>,
    },
    ServerStatus {
        challenge: u32,
        users: u32,
        files: u32,
    },
    Unknown(u8),
}
