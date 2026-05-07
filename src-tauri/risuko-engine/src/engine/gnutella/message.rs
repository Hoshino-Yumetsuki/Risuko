//! Gnutella binary message header (23 bytes)
//!
//! Layout:
//!   0..16  GUID (16 bytes)
//!   16     payload type
//!   17     TTL
//!   18     hops
//!   19..23 payload length (u32 little-endian)

/// Payload-type byte for a `PING` descriptor (servent liveness probe)
pub const PING: u8 = 0x00;
/// Payload-type byte for a `PONG` descriptor (response to PING)
pub const PONG: u8 = 0x01;
/// Payload-type byte for a `BYE` descriptor (graceful disconnect)
pub const BYE: u8 = 0x02;
/// Payload-type byte for a `QUERY` descriptor (content search)
pub const QUERY: u8 = 0x80;
/// Payload-type byte for a `QUERY HIT` descriptor (search response)
pub const QUERYHIT: u8 = 0x81;
/// Payload-type byte for a `PUSH` descriptor (firewalled-source request)
pub const PUSH: u8 = 0x40;

/// Generate a fresh 16-byte message GUID with RFC 4122 variant bits set,
/// reducing the chance of collision against other servents on the network
pub fn make_guid() -> [u8; 16] {
    use rand::RngExt;
    let mut g = [0u8; 16];
    let mut rng = rand::rng();
    rng.fill(&mut g);
    // RFC-4122-style version/variant marks help avoid duplicates
    g[8] = (g[8] & 0x3F) | 0x80;
    g[15] = 0xFF;
    g
}

/// Encode the 23-byte Gnutella descriptor header from its parts. Payload
/// length is written in little-endian per the wire spec
pub fn encode_header(guid: &[u8; 16], typ: u8, ttl: u8, hops: u8, payload_len: u32) -> [u8; 23] {
    let mut hdr = [0u8; 23];
    hdr[0..16].copy_from_slice(guid);
    hdr[16] = typ;
    hdr[17] = ttl;
    hdr[18] = hops;
    hdr[19..23].copy_from_slice(&payload_len.to_le_bytes());
    hdr
}

/// Parse a 23-byte descriptor header. Returns `None` when the buffer is
/// shorter than the fixed header size; does not validate `typ` or payload
pub fn parse_header(buf: &[u8]) -> Option<([u8; 16], u8, u8, u8, u32)> {
    if buf.len() < 23 {
        return None;
    }
    let mut guid = [0u8; 16];
    guid.copy_from_slice(&buf[0..16]);
    let typ = buf[16];
    let ttl = buf[17];
    let hops = buf[18];
    let len = u32::from_le_bytes([buf[19], buf[20], buf[21], buf[22]]);
    Some((guid, typ, ttl, hops, len))
}

/// Build a QUERY payload: `<min-speed u16le> <search-text>\0[hugeext]\0`
pub fn build_query(min_speed: u16, search_text: &str, urn: Option<&str>) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 + search_text.len() + 1 + 64);
    out.extend_from_slice(&min_speed.to_le_bytes());
    out.extend_from_slice(search_text.as_bytes());
    out.push(0);
    if let Some(u) = urn {
        out.extend_from_slice(u.as_bytes());
    }
    out.push(0);
    out
}
