use serde::{Deserialize, Serialize};

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

/// ed2k chunk size: 9,728,000 bytes (9500 KiB)
pub const ED2K_CHUNK_SIZE: u64 = 9_728_000;

/// ed2k protocol identifier
pub const PROTO_EDONKEY: u8 = 0xe3;

/// Client -> Server opcodes
pub const OP_HELLO_SERVER: u8 = 0x01;
pub const OP_OFFER_FILES: u8 = 0x15;
pub const OP_GET_SOURCES: u8 = 0x19;

/// Server -> Client opcodes
pub const OP_ID_CHANGE: u8 = 0x40;
pub const OP_SERVER_MESSAGE: u8 = 0x38;
pub const OP_SERVER_STATUS: u8 = 0x34;
pub const OP_FOUND_SOURCES: u8 = 0x42;
pub const OP_SERVER_LIST: u8 = 0x32;

/// Client -> Client opcodes
pub const OP_HELLO_CLIENT: u8 = 0x01;
pub const OP_HELLO_ANSWER: u8 = 0x4c;
pub const OP_FILE_REQUEST: u8 = 0x58;
pub const OP_FILE_STATUS_REQUEST: u8 = 0x4f;
pub const OP_FILE_STATUS: u8 = 0x50;
pub const OP_HASHSET_REQUEST: u8 = 0x51;
pub const OP_HASHSET_ANSWER: u8 = 0x52;
pub const OP_SLOT_REQUEST: u8 = 0x54;
pub const OP_SLOT_GIVEN: u8 = 0x55;
pub const OP_SLOT_TAKEN: u8 = 0x57;
pub const OP_REQUEST_PARTS: u8 = 0x47;
pub const OP_SENDING_PART: u8 = 0x46;

/// eMule extension opcodes (protocol 0xc5)
pub const OP_EMULE_QUEUE_RANKING: u8 = 0x60;

/// Meta tag special IDs
pub const TAG_NAME: u8 = 0x01;
pub const TAG_PORT: u8 = 0x0f;
pub const TAG_VERSION: u8 = 0x11;

/// eMule meta tag special IDs
pub const TAG_EMULE_UDP_PORT: u8 = 0x21;

/// Client ID threshold for High/Low ID; IDs below this value are "Low ID" (behind NAT)
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
    file_size.div_ceil(ED2K_CHUNK_SIZE)
}
