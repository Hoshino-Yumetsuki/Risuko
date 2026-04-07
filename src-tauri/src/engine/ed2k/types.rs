#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::net::SocketAddrV4;

/// Parsed ed2k file link
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ed2kFileLink {
    pub file_name: String,
    pub file_size: u64,
    /// 16-byte MD4 hash as hex string (32 chars)
    pub file_hash: String,
    /// Raw 16-byte MD4 hash
    #[serde(skip)]
    pub file_hash_bytes: [u8; 16],
    pub sources: Vec<Ed2kSource>,
    /// Optional AICH root hash (base32 encoded)
    pub aich_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ed2kSource {
    pub ip: String,
    pub port: u16,
}

impl Ed2kSource {
    pub fn to_socket_addr(&self) -> Option<SocketAddrV4> {
        let ip = self.ip.parse().ok()?;
        Some(SocketAddrV4::new(ip, self.port))
    }
}

/// ed2k chunk size: 9,728,000 bytes (9500 KiB)
pub const ED2K_CHUNK_SIZE: u64 = 9_728_000;

/// Default ed2k TCP port
pub const ED2K_DEFAULT_PORT: u16 = 4662;

/// Default ed2k UDP port (TCP + 4)
pub const ED2K_DEFAULT_UDP_PORT: u16 = 4666;

/// ed2k protocol identifiers
pub const PROTO_EDONKEY: u8 = 0xe3;
pub const PROTO_EMULE_EXT: u8 = 0xc5;
pub const PROTO_EMULE_COMPRESSED: u8 = 0xd4;

/// Client -> Server opcodes
pub const OP_HELLO_SERVER: u8 = 0x01;
pub const OP_OFFER_FILES: u8 = 0x15;
pub const OP_SEARCH_FILE: u8 = 0x16;
pub const OP_DISCONNECT: u8 = 0x18;
pub const OP_GET_SOURCES: u8 = 0x19;
pub const OP_GET_SERVER_LIST: u8 = 0x14;

/// Server -> Client opcodes
pub const OP_ID_CHANGE: u8 = 0x40;
pub const OP_SERVER_MESSAGE: u8 = 0x38;
pub const OP_SERVER_STATUS: u8 = 0x34;
pub const OP_SERVER_INFO_DATA: u8 = 0x41;
pub const OP_FOUND_SOURCES: u8 = 0x42;
pub const OP_SEARCH_FILE_RESULTS: u8 = 0x33;
pub const OP_SERVER_LIST: u8 = 0x32;
pub const OP_CALLBACK_FAIL: u8 = 0x36;

/// Client -> Client opcodes
pub const OP_HELLO_CLIENT: u8 = 0x01;
pub const OP_HELLO_ANSWER: u8 = 0x4c;
pub const OP_FILE_REQUEST: u8 = 0x58;
pub const OP_FILE_REQUEST_ANSWER: u8 = 0x59;
pub const OP_NO_SUCH_FILE: u8 = 0x48;
pub const OP_FILE_STATUS_REQUEST: u8 = 0x4f;
pub const OP_FILE_STATUS: u8 = 0x50;
pub const OP_HASHSET_REQUEST: u8 = 0x51;
pub const OP_HASHSET_ANSWER: u8 = 0x52;
pub const OP_SLOT_REQUEST: u8 = 0x54;
pub const OP_SLOT_GIVEN: u8 = 0x55;
pub const OP_SLOT_RELEASE: u8 = 0x56;
pub const OP_SLOT_TAKEN: u8 = 0x57;
pub const OP_REQUEST_PARTS: u8 = 0x47;
pub const OP_SENDING_PART: u8 = 0x46;
pub const OP_END_OF_DOWNLOAD: u8 = 0x49;

/// eMule extension opcodes (protocol 0xc5)
pub const OP_EMULE_HELLO: u8 = 0x01;
pub const OP_EMULE_HELLO_ANSWER: u8 = 0x02;
pub const OP_EMULE_DATA_COMPRESSED: u8 = 0x40;
pub const OP_EMULE_QUEUE_RANKING: u8 = 0x60;
pub const OP_EMULE_SOURCES_REQUEST: u8 = 0x81;
pub const OP_EMULE_SOURCES_ANSWER: u8 = 0x82;

/// UDP opcodes
pub const OP_UDP_SERVER_STATUS_REQ: u8 = 0x96;
pub const OP_UDP_SERVER_STATUS: u8 = 0x97;
pub const OP_UDP_SEARCH_FILE: u8 = 0x98;
pub const OP_UDP_SEARCH_FILE_RESULT: u8 = 0x99;
pub const OP_UDP_GET_SOURCES: u8 = 0x9a;
pub const OP_UDP_FOUND_SOURCES: u8 = 0x9b;

/// Meta tag special IDs
pub const TAG_NAME: u8 = 0x01;
pub const TAG_SIZE: u8 = 0x02;
pub const TAG_TYPE: u8 = 0x03;
pub const TAG_FORMAT: u8 = 0x04;
pub const TAG_PORT: u8 = 0x0f;
pub const TAG_IP: u8 = 0x10;
pub const TAG_VERSION: u8 = 0x11;

/// eMule meta tag special IDs
pub const TAG_EMULE_COMPRESSION: u8 = 0x20;
pub const TAG_EMULE_UDP_PORT: u8 = 0x21;
pub const TAG_EMULE_UDP_VERSION: u8 = 0x22;
pub const TAG_EMULE_SOURCE_EXCHANGE: u8 = 0x23;
pub const TAG_EMULE_COMMENTS: u8 = 0x24;
pub const TAG_EMULE_EXTENDED_REQUEST: u8 = 0x25;
pub const TAG_EMULE_COMPATIBLE_CLIENT: u8 = 0x26;

/// Client ID threshold for High/Low ID
/// IDs below this value are "Low ID" (behind NAT)
pub const LOW_ID_THRESHOLD: u32 = 16_777_216; // 0x01000000

/// Check if a client ID is a High ID (directly reachable)
pub fn is_high_id(client_id: u32) -> bool {
    client_id >= LOW_ID_THRESHOLD
}

/// Convert a High ID to an IP address (stored in little-endian: 0xAABBCCDD -> DD.CC.BB.AA)
pub fn client_id_to_ip(client_id: u32) -> std::net::Ipv4Addr {
    let bytes = client_id.to_le_bytes();
    std::net::Ipv4Addr::new(bytes[0], bytes[1], bytes[2], bytes[3])
}

/// Server information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    pub ip: String,
    pub port: u16,
    pub name: String,
    pub description: String,
    pub users: u32,
    pub files: u32,
    pub max_users: u32,
    pub priority: ServerPriority,
    pub ping_ms: Option<u32>,
    pub last_seen: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerPriority {
    Low,
    Normal,
    High,
}

impl Default for ServerPriority {
    fn default() -> Self {
        Self::Normal
    }
}

/// Chunk status during download
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkStatus {
    Missing,
    Downloaded,
}

/// Calculate the number of chunks for a given file size
pub fn chunk_count(file_size: u64) -> u64 {
    if file_size == 0 {
        return 0;
    }
    (file_size + ED2K_CHUNK_SIZE - 1) / ED2K_CHUNK_SIZE
}
