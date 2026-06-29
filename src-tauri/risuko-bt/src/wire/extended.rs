//! BEP-10 extension protocol: handshake dict + ut_metadata (BEP-9) and
//! ut_pex (BEP-11) payloads
//!
//! On the wire an extended message is framed as a normal `Message::Extended`
//! with `ext_id == 0` identifying the extended-handshake; higher ids map to
//! per-peer extension message types negotiated via the handshake's `m` dict

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use bytes::Bytes;

use super::super::bencode::{decode_all, encode_to_vec, Value};

pub const EXT_HANDSHAKE_ID: u8 = 0;

pub const EXT_NAME_UT_METADATA: &[u8] = b"ut_metadata";
pub const EXT_NAME_UT_PEX: &[u8] = b"ut_pex";
pub const EXT_NAME_UT_HOLEPUNCH: &[u8] = b"ut_holepunch";

/// ut_metadata message types (BEP-9)
pub mod ut_metadata_type {
    pub const REQUEST: i64 = 0;
    pub const DATA: i64 = 1;
    pub const REJECT: i64 = 2;
}

/// ut_holepunch (BEP-55) message types. Unlike ut_metadata/ut_pex these are a
/// fixed binary layout, NOT bencoded: `msg_type(1) addr_type(1) ip(4|16)
/// port(2) [err_code(4) for Error]`
pub mod holepunch_type {
    /// Initiator -> relay: "help me reach this endpoint"
    pub const RENDEZVOUS: u8 = 0;
    /// Relay -> both ends: "connect to this endpoint now" (simultaneous open)
    pub const CONNECT: u8 = 1;
    /// Relay -> initiator: rendezvous failed
    pub const ERROR: u8 = 2;
}

/// ut_holepunch (BEP-55) error codes carried by `holepunch_type::ERROR`
pub mod holepunch_err {
    /// The relay is not connected to the requested target
    pub const NO_SUCH_PEER: u32 = 1;
    /// The relay is connected to the target but in a state that can't relay
    pub const NOT_CONNECTED: u32 = 2;
    /// The target does not advertise ut_holepunch
    pub const NO_SUPPORT: u32 = 3;
    /// The target endpoint is the relay itself
    pub const NO_SELF: u32 = 4;
}

#[derive(Debug, Clone, Default)]
pub struct ExtHandshake {
    /// Map of extension name -> per-peer message id
    pub supported: HashMap<Vec<u8>, u8>,
    /// Total size of the `info` dict, if the peer advertises it (BEP-9)
    pub metadata_size: Option<u64>,
    /// Peer-advertised client string ("v" key)
    pub client: Option<String>,
    /// BEP-10 `yourip`: the peer's public address as we observed it. Some real-world
    /// clients (notably some CN BT implementations) only engage with a remote that
    /// echoes their address back here
    pub yourip: Option<IpAddr>,
}

impl ExtHandshake {
    /// Build our outgoing extended handshake. Caller supplies the message ids
    /// we want peers to use when sending us the respective extension
    pub fn new_outgoing(ut_metadata_id: u8, ut_pex_id: u8, metadata_size: Option<u64>) -> Self {
        let mut supported = HashMap::new();
        supported.insert(EXT_NAME_UT_METADATA.to_vec(), ut_metadata_id);
        supported.insert(EXT_NAME_UT_PEX.to_vec(), ut_pex_id);
        Self {
            supported,
            metadata_size,
            client: Some(format!(
                "{} {}",
                env!("CARGO_PKG_NAME"),
                env!("CARGO_PKG_VERSION")
            )),
            yourip: None,
        }
    }

    /// Set `yourip` to the peer's address (compact-encoded on the wire). Builder
    /// helper used by the connection layer per dial / accept
    pub fn with_yourip(mut self, ip: IpAddr) -> Self {
        self.yourip = Some(ip);
        self
    }

    /// Advertise `ut_holepunch` (BEP-55) with the message id we want peers to
    /// use when sending us holepunch messages. Builder so callers can opt in
    /// without churning the `new_outgoing` signature
    pub fn with_holepunch(mut self, ut_holepunch_id: u8) -> Self {
        self.supported
            .insert(EXT_NAME_UT_HOLEPUNCH.to_vec(), ut_holepunch_id);
        self
    }

    pub fn encode(&self) -> Bytes {
        let mut m_entries: Vec<(Vec<u8>, Value)> = self
            .supported
            .iter()
            .map(|(k, v)| (k.clone(), Value::Int(*v as i64)))
            .collect();
        m_entries.sort_by(|a, b| a.0.cmp(&b.0));
        // Bencode dictionaries must be lexicographically sorted by key. The keys we
        // emit are `m`, `metadata_size`, `v`, `yourip`—all distinct and already in
        // sorted order, so we just push in that fixed sequence
        let mut dict = vec![(b"m".to_vec(), Value::Dict(m_entries))];
        if let Some(sz) = self.metadata_size {
            dict.push((b"metadata_size".to_vec(), Value::Int(sz as i64)));
        }
        if let Some(v) = &self.client {
            dict.push((b"v".to_vec(), Value::Bytes(v.as_bytes().to_vec())));
        }
        if let Some(ip) = &self.yourip {
            let bytes = match ip {
                IpAddr::V4(v4) => v4.octets().to_vec(),
                IpAddr::V6(v6) => v6.octets().to_vec(),
            };
            dict.push((b"yourip".to_vec(), Value::Bytes(bytes)));
        }
        Bytes::from(encode_to_vec(&Value::Dict(dict)))
    }

    pub fn decode(payload: &[u8]) -> Option<Self> {
        let value = decode_all(payload).ok()?;
        let dict = value.as_dict()?;
        let mut supported = HashMap::new();
        if let Some((_, m)) = dict.iter().find(|(k, _)| k == b"m") {
            if let Some(m_dict) = m.as_dict() {
                for (k, v) in m_dict {
                    if let Some(id) = v.as_int() {
                        // Extension ID 0 is reserved for the BEP-10 handshake itself
                        // and must not appear in the `m` dict.
                        if (1..=255).contains(&id) {
                            supported.insert(k.clone(), id as u8);
                        }
                    }
                }
            }
        }
        let metadata_size = dict
            .iter()
            .find(|(k, _)| k == b"metadata_size")
            .and_then(|(_, v)| v.as_int())
            .and_then(|n| if n >= 0 { Some(n as u64) } else { None });
        let client = dict
            .iter()
            .find(|(k, _)| k == b"v")
            .and_then(|(_, v)| v.as_str().map(String::from));
        let yourip = dict
            .iter()
            .find(|(k, _)| k == b"yourip")
            .and_then(|(_, v)| v.as_bytes())
            .and_then(|bytes| match bytes.len() {
                4 => {
                    let arr: [u8; 4] = bytes.try_into().ok()?;
                    Some(IpAddr::V4(std::net::Ipv4Addr::from(arr)))
                }
                16 => {
                    let arr: [u8; 16] = bytes.try_into().ok()?;
                    Some(IpAddr::V6(std::net::Ipv6Addr::from(arr)))
                }
                _ => None,
            });
        Some(Self {
            supported,
            metadata_size,
            client,
            yourip,
        })
    }

    pub fn ut_metadata_id(&self) -> Option<u8> {
        self.supported.get(EXT_NAME_UT_METADATA).copied()
    }

    pub fn ut_pex_id(&self) -> Option<u8> {
        self.supported.get(EXT_NAME_UT_PEX).copied()
    }

    pub fn ut_holepunch_id(&self) -> Option<u8> {
        self.supported.get(EXT_NAME_UT_HOLEPUNCH).copied()
    }
}

/// Build a `ut_metadata` request for a given piece index
pub fn ut_metadata_request(piece: i64) -> Bytes {
    let dict = Value::Dict(vec![
        (b"msg_type".to_vec(), Value::Int(ut_metadata_type::REQUEST)),
        (b"piece".to_vec(), Value::Int(piece)),
    ]);
    Bytes::from(encode_to_vec(&dict))
}

/// Build a `ut_metadata` data response
pub fn ut_metadata_data(piece: i64, total_size: i64, block: &[u8]) -> Bytes {
    let header = Value::Dict(vec![
        (b"msg_type".to_vec(), Value::Int(ut_metadata_type::DATA)),
        (b"piece".to_vec(), Value::Int(piece)),
        (b"total_size".to_vec(), Value::Int(total_size)),
    ]);
    let mut out = encode_to_vec(&header);
    out.extend_from_slice(block);
    Bytes::from(out)
}

/// Build a `ut_metadata` reject response (sent for out-of-range pieces or
/// when we cannot serve the request)
pub fn ut_metadata_reject(piece: i64) -> Bytes {
    let dict = Value::Dict(vec![
        (b"msg_type".to_vec(), Value::Int(ut_metadata_type::REJECT)),
        (b"piece".to_vec(), Value::Int(piece)),
    ]);
    Bytes::from(encode_to_vec(&dict))
}

/// Parse a `ut_metadata` message, returning the parsed header and any trailing
/// data block (for DATA messages)
pub struct UtMetadataMsg {
    pub msg_type: i64,
    pub piece: i64,
    pub total_size: Option<i64>,
    pub block: Bytes,
}

pub fn parse_ut_metadata(payload: Bytes) -> Option<UtMetadataMsg> {
    // ut_metadata messages carry a bencoded dict followed by the raw data for
    // DATA messages. We need to know how many bytes the dict consumed
    let mut p = crate::bencode::decode(&payload).ok()?;
    let block = payload.slice(p.span.end..);
    let dict = match &mut p.value {
        Value::Dict(d) => std::mem::take(&mut *d),
        _ => return None,
    };
    let msg_type = dict
        .iter()
        .find(|(k, _)| k == b"msg_type")
        .and_then(|(_, v)| v.as_int())?;
    let piece = dict
        .iter()
        .find(|(k, _)| k == b"piece")
        .and_then(|(_, v)| v.as_int())?;
    let total_size = dict
        .iter()
        .find(|(k, _)| k == b"total_size")
        .and_then(|(_, v)| v.as_int());
    Some(UtMetadataMsg {
        msg_type,
        piece,
        total_size,
        block,
    })
}

/// Decode a compact ut_pex payload. Returns (ipv4 peers, ipv6 peers). Each
/// peer is a (IP, port) pair. Only IPv4 is currently wired up to the session
pub fn parse_ut_pex(
    payload: &[u8],
) -> Option<(Vec<std::net::SocketAddr>, Vec<std::net::SocketAddr>)> {
    let value = decode_all(payload).ok()?;
    let dict = value.as_dict()?;
    let mut v4 = Vec::new();
    let mut v6 = Vec::new();
    if let Some(added) = dict
        .iter()
        .find(|(k, _)| k == b"added")
        .and_then(|(_, v)| v.as_bytes())
    {
        for chunk in added.chunks_exact(6) {
            let ip = std::net::Ipv4Addr::new(chunk[0], chunk[1], chunk[2], chunk[3]);
            let port = u16::from_be_bytes([chunk[4], chunk[5]]);
            v4.push(std::net::SocketAddr::from((ip, port)));
        }
    }
    if let Some(added6) = dict
        .iter()
        .find(|(k, _)| k == b"added6")
        .and_then(|(_, v)| v.as_bytes())
    {
        for chunk in added6.chunks_exact(18) {
            let mut octets = [0u8; 16];
            octets.copy_from_slice(&chunk[..16]);
            let ip = std::net::Ipv6Addr::from(octets);
            let port = u16::from_be_bytes([chunk[16], chunk[17]]);
            v6.push(std::net::SocketAddr::from((ip, port)));
        }
    }
    Some((v4, v6))
}

/// A decoded BEP-55 ut_holepunch message
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HolepunchMsg {
    /// One of [`holepunch_type`]
    pub msg_type: u8,
    /// The endpoint the message refers to (rendezvous target / connect peer)
    pub addr: SocketAddr,
    /// One of [`holepunch_err`]; meaningful only for `holepunch_type::ERROR`
    pub err_code: u32,
}

/// Encode a BEP-55 ut_holepunch message. `err_code` is only emitted for
/// `holepunch_type::ERROR`
pub fn build_holepunch(msg_type: u8, addr: SocketAddr, err_code: u32) -> Bytes {
    let mut buf = Vec::with_capacity(24);
    buf.push(msg_type);
    match addr.ip() {
        IpAddr::V4(v4) => {
            buf.push(0);
            buf.extend_from_slice(&v4.octets());
        }
        IpAddr::V6(v6) => {
            buf.push(1);
            buf.extend_from_slice(&v6.octets());
        }
    }
    buf.extend_from_slice(&addr.port().to_be_bytes());
    if msg_type == holepunch_type::ERROR {
        buf.extend_from_slice(&err_code.to_be_bytes());
    }
    Bytes::from(buf)
}

/// Decode a BEP-55 ut_holepunch message. Returns `None` on a malformed/short
/// payload or an unknown address family
pub fn parse_holepunch(payload: &[u8]) -> Option<HolepunchMsg> {
    if payload.len() < 2 {
        return None;
    }
    let msg_type = payload[0];
    let addr_type = payload[1];
    let (ip, port_off): (IpAddr, usize) = match addr_type {
        0 => {
            if payload.len() < 2 + 4 + 2 {
                return None;
            }
            let arr: [u8; 4] = payload[2..6].try_into().ok()?;
            (IpAddr::V4(Ipv4Addr::from(arr)), 6)
        }
        1 => {
            if payload.len() < 2 + 16 + 2 {
                return None;
            }
            let arr: [u8; 16] = payload[2..18].try_into().ok()?;
            (IpAddr::V6(Ipv6Addr::from(arr)), 18)
        }
        _ => return None,
    };
    let port = u16::from_be_bytes([payload[port_off], payload[port_off + 1]]);
    let mut err_code = 0u32;
    if msg_type == holepunch_type::ERROR {
        let eo = port_off + 2;
        if payload.len() < eo + 4 {
            return None;
        }
        err_code = u32::from_be_bytes(payload[eo..eo + 4].try_into().ok()?);
    }
    Some(HolepunchMsg {
        msg_type,
        addr: SocketAddr::new(ip, port),
        err_code,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_round_trip() {
        let out = ExtHandshake::new_outgoing(3, 4, Some(1024));
        let bytes = out.encode();
        let parsed = ExtHandshake::decode(&bytes).unwrap();
        assert_eq!(parsed.ut_metadata_id(), Some(3));
        assert_eq!(parsed.ut_pex_id(), Some(4));
        assert_eq!(parsed.metadata_size, Some(1024));
        assert!(parsed.client.is_some());
    }

    #[test]
    fn ut_metadata_request_parse() {
        let bytes = ut_metadata_request(5);
        let parsed = parse_ut_metadata(bytes).unwrap();
        assert_eq!(parsed.msg_type, ut_metadata_type::REQUEST);
        assert_eq!(parsed.piece, 5);
        assert!(parsed.block.is_empty());
    }

    #[test]
    fn ut_metadata_data_parse() {
        let data = vec![0xaau8; 200];
        let bytes = ut_metadata_data(2, 1_000_000, &data);
        let parsed = parse_ut_metadata(bytes).unwrap();
        assert_eq!(parsed.msg_type, ut_metadata_type::DATA);
        assert_eq!(parsed.piece, 2);
        assert_eq!(parsed.total_size, Some(1_000_000));
        assert_eq!(parsed.block.as_ref(), data.as_slice());
    }

    #[test]
    fn ut_metadata_reject_parse() {
        let bytes = ut_metadata_reject(7);
        let parsed = parse_ut_metadata(bytes).unwrap();
        assert_eq!(parsed.msg_type, ut_metadata_type::REJECT);
        assert_eq!(parsed.piece, 7);
        assert!(parsed.block.is_empty());
    }

    #[test]
    fn yourip_ipv4_round_trip() {
        let ip = std::net::IpAddr::V4("192.168.1.42".parse().unwrap());
        let out = ExtHandshake::new_outgoing(3, 4, None).with_yourip(ip);
        let bytes = out.encode();
        let parsed = ExtHandshake::decode(&bytes).unwrap();
        assert_eq!(parsed.yourip, Some(ip));
    }

    #[test]
    fn yourip_ipv6_round_trip() {
        let ip = std::net::IpAddr::V6("2001:db8::1".parse().unwrap());
        let out = ExtHandshake::new_outgoing(3, 4, None).with_yourip(ip);
        let bytes = out.encode();
        let parsed = ExtHandshake::decode(&bytes).unwrap();
        assert_eq!(parsed.yourip, Some(ip));
    }

    #[test]
    fn holepunch_advertised_in_handshake() {
        let out = ExtHandshake::new_outgoing(3, 4, None).with_holepunch(5);
        let bytes = out.encode();
        let parsed = ExtHandshake::decode(&bytes).unwrap();
        assert_eq!(parsed.ut_holepunch_id(), Some(5));
        // Existing extensions remain advertised alongside it
        assert_eq!(parsed.ut_metadata_id(), Some(3));
        assert_eq!(parsed.ut_pex_id(), Some(4));
    }

    #[test]
    fn holepunch_handshake_absent_when_not_advertised() {
        let out = ExtHandshake::new_outgoing(3, 4, None);
        let parsed = ExtHandshake::decode(&out.encode()).unwrap();
        assert_eq!(parsed.ut_holepunch_id(), None);
    }

    #[test]
    fn holepunch_connect_v4_round_trip() {
        let addr: SocketAddr = "203.0.113.7:51413".parse().unwrap();
        let bytes = build_holepunch(holepunch_type::CONNECT, addr, 0);
        // type(1) + addr_type(1) + ipv4(4) + port(2), no err_code for non-error
        assert_eq!(bytes.len(), 8);
        let parsed = parse_holepunch(&bytes).unwrap();
        assert_eq!(parsed.msg_type, holepunch_type::CONNECT);
        assert_eq!(parsed.addr, addr);
        assert_eq!(parsed.err_code, 0);
    }

    #[test]
    fn holepunch_rendezvous_v6_round_trip() {
        let addr: SocketAddr = "[2001:db8::dead:beef]:6881".parse().unwrap();
        let bytes = build_holepunch(holepunch_type::RENDEZVOUS, addr, 0);
        // type(1) + addr_type(1) + ipv6(16) + port(2)
        assert_eq!(bytes.len(), 20);
        let parsed = parse_holepunch(&bytes).unwrap();
        assert_eq!(parsed.msg_type, holepunch_type::RENDEZVOUS);
        assert_eq!(parsed.addr, addr);
    }

    #[test]
    fn holepunch_error_carries_code() {
        let addr: SocketAddr = "198.51.100.9:1337".parse().unwrap();
        let bytes = build_holepunch(holepunch_type::ERROR, addr, holepunch_err::NO_SUCH_PEER);
        assert_eq!(bytes.len(), 12); // ...+ err_code(4)
        let parsed = parse_holepunch(&bytes).unwrap();
        assert_eq!(parsed.msg_type, holepunch_type::ERROR);
        assert_eq!(parsed.addr, addr);
        assert_eq!(parsed.err_code, holepunch_err::NO_SUCH_PEER);
    }

    #[test]
    fn holepunch_rejects_truncated_and_unknown_family() {
        assert!(parse_holepunch(&[]).is_none());
        assert!(parse_holepunch(&[holepunch_type::CONNECT]).is_none());
        // addr_type 0 (v4) but missing port bytes
        assert!(parse_holepunch(&[holepunch_type::CONNECT, 0, 1, 2, 3, 4]).is_none());
        // unknown address family
        assert!(parse_holepunch(&[holepunch_type::CONNECT, 9, 1, 2, 3, 4, 0, 0]).is_none());
    }
}
