#![allow(dead_code)]

use super::types::*;
use bytes::{Buf, BytesMut};
use std::io;

/// An ed2k packet: protocol byte + opcode + payload
#[derive(Debug, Clone)]
pub struct Ed2kPacket {
    pub protocol: u8,
    pub opcode: u8,
    pub payload: Vec<u8>,
}

impl Ed2kPacket {
    pub fn new(protocol: u8, opcode: u8, payload: Vec<u8>) -> Self {
        Self {
            protocol,
            opcode,
            payload,
        }
    }

    /// Encode this packet into bytes for sending over the wire
    pub fn encode(&self) -> Vec<u8> {
        // Format: [protocol:1][length:4(LE)][opcode:1][payload:N]
        // length = 1 (opcode) + payload.len()
        let data_len = 1 + self.payload.len();
        let mut buf = Vec::with_capacity(5 + data_len);
        buf.push(self.protocol);
        buf.extend_from_slice(&(data_len as u32).to_le_bytes());
        buf.push(self.opcode);
        buf.extend_from_slice(&self.payload);
        buf
    }

    /// Try to decode a packet from a byte buffer. Returns None if not enough data
    pub fn decode(buf: &mut BytesMut) -> Result<Option<Self>, io::Error> {
        if buf.len() < 5 {
            return Ok(None);
        }

        let protocol = buf[0];
        let data_len = u32::from_le_bytes([buf[1], buf[2], buf[3], buf[4]]) as usize;

        if data_len == 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "Zero-length ed2k packet"));
        }

        let total_len = 5 + data_len;
        if buf.len() < total_len {
            return Ok(None);
        }

        buf.advance(5);
        let opcode = buf[0];
        buf.advance(1);

        let payload_len = data_len - 1;
        let payload = buf.split_to(payload_len).to_vec();

        Ok(Some(Self {
            protocol,
            opcode,
            payload,
        }))
    }
}

// ── Packet builders ─────────────────────────────────────────────────

/// Build a Hello Server packet (opcode 0x01)
/// Contains: client_hash(16) + client_id(4) + port(2) + meta_tags
pub fn build_hello_server(
    client_hash: &[u8; 16],
    client_port: u16,
) -> Ed2kPacket {
    let mut payload = Vec::with_capacity(64);
    payload.extend_from_slice(client_hash);
    payload.extend_from_slice(&0u32.to_le_bytes()); // client_id = 0 (connecting)
    payload.extend_from_slice(&client_port.to_le_bytes());

    // Meta tags: name, version, port
    let tags = vec![
        MetaTag::string(TAG_NAME, "Motrix"),
        MetaTag::u32(TAG_VERSION, 0x3c), // version 60
        MetaTag::u32(TAG_PORT, client_port as u32),
        MetaTag::u32(TAG_EMULE_UDP_PORT, (client_port as u32) + 4),
    ];

    let tag_count = tags.len() as u32;
    payload.extend_from_slice(&tag_count.to_le_bytes());
    for tag in &tags {
        tag.encode(&mut payload);
    }

    Ed2kPacket::new(PROTO_EDONKEY, OP_HELLO_SERVER, payload)
}

/// Build a Get Sources packet (opcode 0x19)
pub fn build_get_sources(file_hash: &[u8; 16]) -> Ed2kPacket {
    Ed2kPacket::new(PROTO_EDONKEY, OP_GET_SOURCES, file_hash.to_vec())
}

/// Build an Offer Files packet (opcode 0x15) — empty file list
pub fn build_offer_files_empty() -> Ed2kPacket {
    let mut payload = Vec::new();
    payload.extend_from_slice(&0u32.to_le_bytes()); // 0 files
    Ed2kPacket::new(PROTO_EDONKEY, OP_OFFER_FILES, payload)
}

/// Build a Get Server List packet (opcode 0x14)
pub fn build_get_server_list() -> Ed2kPacket {
    Ed2kPacket::new(PROTO_EDONKEY, OP_GET_SERVER_LIST, vec![])
}

/// Build UDP Get Sources packet (opcode 0x9a)
pub fn build_udp_get_sources(file_hash: &[u8; 16]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(17);
    buf.push(PROTO_EDONKEY);
    buf.push(OP_UDP_GET_SOURCES);
    buf.extend_from_slice(file_hash);
    buf
}

/// Build UDP Server Status Request (opcode 0x96)
pub fn build_udp_server_status_req(challenge: u32) -> Vec<u8> {
    let mut buf = Vec::with_capacity(6);
    buf.push(PROTO_EDONKEY);
    buf.push(OP_UDP_SERVER_STATUS_REQ);
    buf.extend_from_slice(&challenge.to_le_bytes());
    buf
}

// ── Packet parsers ──────────────────────────────────────────────────

/// Parse ID Change packet (opcode 0x40): returns client_id
pub fn parse_id_change(payload: &[u8]) -> Result<u32, String> {
    if payload.len() < 4 {
        return Err("ID Change packet too short".to_string());
    }
    Ok(u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]))
}

/// Parse Server Status packet (opcode 0x34): returns (users, files)
pub fn parse_server_status(payload: &[u8]) -> Result<(u32, u32), String> {
    if payload.len() < 8 {
        return Err("Server Status packet too short".to_string());
    }
    let users = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
    let files = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
    Ok((users, files))
}

/// Parse Server Message packet (opcode 0x38): returns message string
pub fn parse_server_message(payload: &[u8]) -> Result<String, String> {
    if payload.len() < 2 {
        return Err("Server Message packet too short".to_string());
    }
    let len = u16::from_le_bytes([payload[0], payload[1]]) as usize;
    if payload.len() < 2 + len {
        return Err("Server Message truncated".to_string());
    }
    String::from_utf8(payload[2..2 + len].to_vec())
        .map_err(|_| "Server Message contains invalid UTF-8".to_string())
}

/// Parse Found Sources packet (opcode 0x42): returns (file_hash, list of (ip:u32, port:u16))
pub fn parse_found_sources(payload: &[u8]) -> Result<([u8; 16], Vec<(u32, u16)>), String> {
    if payload.len() < 17 {
        return Err("Found Sources packet too short".to_string());
    }

    let mut hash = [0u8; 16];
    hash.copy_from_slice(&payload[0..16]);

    let count = payload[16] as usize;
    let expected_len = 17 + count * 6;
    if payload.len() < expected_len {
        return Err("Found Sources truncated".to_string());
    }

    let mut sources = Vec::with_capacity(count);
    let mut offset = 17;
    for _ in 0..count {
        let ip = u32::from_le_bytes([
            payload[offset],
            payload[offset + 1],
            payload[offset + 2],
            payload[offset + 3],
        ]);
        let port = u16::from_le_bytes([payload[offset + 4], payload[offset + 5]]);
        sources.push((ip, port));
        offset += 6;
    }

    Ok((hash, sources))
}

/// Parse Server List packet (opcode 0x32): returns list of (ip, port)
pub fn parse_server_list(payload: &[u8]) -> Result<Vec<(u32, u16)>, String> {
    if payload.len() < 1 {
        return Err("Server List packet too short".to_string());
    }

    let count = payload[0] as usize;
    let expected_len = 1 + count * 6;
    if payload.len() < expected_len {
        return Err("Server List truncated".to_string());
    }

    let mut servers = Vec::with_capacity(count);
    let mut offset = 1;
    for _ in 0..count {
        let ip = u32::from_le_bytes([
            payload[offset],
            payload[offset + 1],
            payload[offset + 2],
            payload[offset + 3],
        ]);
        let port = u16::from_le_bytes([payload[offset + 4], payload[offset + 5]]);
        servers.push((ip, port));
        offset += 6;
    }

    Ok(servers)
}

/// Parse UDP Found Sources (opcode 0x9b)
pub fn parse_udp_found_sources(data: &[u8]) -> Result<([u8; 16], Vec<(u32, u16)>), String> {
    // Same format as TCP found sources, minus the protocol/length header
    parse_found_sources(data)
}

/// Parse Hashset Answer (opcode 0x52): returns (file_hash, chunk_hashes)
pub fn parse_hashset_answer(payload: &[u8]) -> Result<([u8; 16], Vec<[u8; 16]>), String> {
    if payload.len() < 18 {
        return Err("Hashset Answer too short".to_string());
    }

    let mut file_hash = [0u8; 16];
    file_hash.copy_from_slice(&payload[0..16]);

    let count = u16::from_le_bytes([payload[16], payload[17]]) as usize;
    let expected = 18 + count * 16;
    if payload.len() < expected {
        return Err("Hashset Answer truncated".to_string());
    }

    let mut hashes = Vec::with_capacity(count);
    let mut offset = 18;
    for _ in 0..count {
        let mut h = [0u8; 16];
        h.copy_from_slice(&payload[offset..offset + 16]);
        hashes.push(h);
        offset += 16;
    }

    Ok((file_hash, hashes))
}

/// Parse File Status (opcode 0x50): returns (file_hash, part_bitmap)
pub fn parse_file_status(payload: &[u8]) -> Result<([u8; 16], Vec<bool>), String> {
    if payload.len() < 18 {
        return Err("File Status too short".to_string());
    }

    let mut file_hash = [0u8; 16];
    file_hash.copy_from_slice(&payload[0..16]);

    let part_count = u16::from_le_bytes([payload[16], payload[17]]) as usize;
    let byte_count = (part_count + 7) / 8;
    if payload.len() < 18 + byte_count {
        return Err("File Status bitmap truncated".to_string());
    }

    let mut parts = Vec::with_capacity(part_count);
    for i in 0..part_count {
        let byte_idx = i / 8;
        let bit_idx = i % 8;
        let has_part = (payload[18 + byte_idx] >> bit_idx) & 1 == 1;
        parts.push(has_part);
    }

    Ok((file_hash, parts))
}

/// Parse Sending Part header (opcode 0x46): returns (file_hash, start_offset, end_offset)
pub fn parse_sending_part_header(payload: &[u8]) -> Result<([u8; 16], u32, u32), String> {
    if payload.len() < 24 {
        return Err("Sending Part header too short".to_string());
    }

    let mut file_hash = [0u8; 16];
    file_hash.copy_from_slice(&payload[0..16]);

    let start = u32::from_le_bytes([payload[16], payload[17], payload[18], payload[19]]);
    let end = u32::from_le_bytes([payload[20], payload[21], payload[22], payload[23]]);

    Ok((file_hash, start, end))
}

// ── Meta Tags ───────────────────────────────────────────────────────

/// A simple meta tag for the ed2k protocol
#[derive(Debug, Clone)]
pub enum MetaTag {
    String { name_id: u8, value: String },
    U32 { name_id: u8, value: u32 },
}

impl MetaTag {
    pub fn string(name_id: u8, value: &str) -> Self {
        Self::String {
            name_id,
            value: value.to_string(),
        }
    }

    pub fn u32(name_id: u8, value: u32) -> Self {
        Self::U32 { name_id, value }
    }

    pub fn encode(&self, buf: &mut Vec<u8>) {
        match self {
            Self::String { name_id, value } => {
                buf.push(0x02); // string tag type
                buf.extend_from_slice(&1u16.to_le_bytes()); // name length
                buf.push(*name_id);
                let bytes = value.as_bytes();
                buf.extend_from_slice(&(bytes.len() as u16).to_le_bytes());
                buf.extend_from_slice(bytes);
            }
            Self::U32 { name_id, value } => {
                buf.push(0x03); // u32 tag type
                buf.extend_from_slice(&1u16.to_le_bytes()); // name length
                buf.push(*name_id);
                buf.extend_from_slice(&value.to_le_bytes());
            }
        }
    }
}

// ── Client-to-Client packet builders ────────────────────────────────

/// Build Hello Client packet (opcode 0x01)
pub fn build_hello_client(
    client_hash: &[u8; 16],
    client_id: u32,
    client_port: u16,
    server_ip: u32,
    server_port: u16,
) -> Ed2kPacket {
    let mut payload = Vec::with_capacity(64);
    payload.push(0x10); // hash size (16)
    payload.extend_from_slice(client_hash);
    payload.extend_from_slice(&client_id.to_le_bytes());
    payload.extend_from_slice(&client_port.to_le_bytes());

    let tags = vec![
        MetaTag::string(TAG_NAME, "Motrix"),
        MetaTag::u32(TAG_VERSION, 0x3c),
        MetaTag::u32(TAG_PORT, client_port as u32),
    ];

    let tag_count = tags.len() as u32;
    payload.extend_from_slice(&tag_count.to_le_bytes());
    for tag in &tags {
        tag.encode(&mut payload);
    }

    // Server address
    payload.extend_from_slice(&server_ip.to_le_bytes());
    payload.extend_from_slice(&server_port.to_le_bytes());

    Ed2kPacket::new(PROTO_EDONKEY, OP_HELLO_CLIENT, payload)
}

/// Build File Request (opcode 0x58)
pub fn build_file_request(file_hash: &[u8; 16]) -> Ed2kPacket {
    Ed2kPacket::new(PROTO_EDONKEY, OP_FILE_REQUEST, file_hash.to_vec())
}

/// Build File Status Request (opcode 0x4f)
pub fn build_file_status_request(file_hash: &[u8; 16]) -> Ed2kPacket {
    Ed2kPacket::new(PROTO_EDONKEY, OP_FILE_STATUS_REQUEST, file_hash.to_vec())
}

/// Build Hashset Request (opcode 0x51)
pub fn build_hashset_request(file_hash: &[u8; 16]) -> Ed2kPacket {
    Ed2kPacket::new(PROTO_EDONKEY, OP_HASHSET_REQUEST, file_hash.to_vec())
}

/// Build Slot Request (opcode 0x54)
pub fn build_slot_request(file_hash: &[u8; 16]) -> Ed2kPacket {
    Ed2kPacket::new(PROTO_EDONKEY, OP_SLOT_REQUEST, file_hash.to_vec())
}

/// Build Request Parts (opcode 0x47): request up to 3 ranges
pub fn build_request_parts(
    file_hash: &[u8; 16],
    ranges: &[(u32, u32)],
) -> Ed2kPacket {
    let mut payload = Vec::with_capacity(16 + 24);
    payload.extend_from_slice(file_hash);

    // 3 start offsets, then 3 end offsets
    for i in 0..3 {
        let start = ranges.get(i).map(|r| r.0).unwrap_or(0);
        payload.extend_from_slice(&start.to_le_bytes());
    }
    for i in 0..3 {
        let end = ranges.get(i).map(|r| r.1).unwrap_or(0);
        payload.extend_from_slice(&end.to_le_bytes());
    }

    Ed2kPacket::new(PROTO_EDONKEY, OP_REQUEST_PARTS, payload)
}

/// Build Slot Release (opcode 0x56)
pub fn build_slot_release() -> Ed2kPacket {
    Ed2kPacket::new(PROTO_EDONKEY, OP_SLOT_RELEASE, vec![])
}
