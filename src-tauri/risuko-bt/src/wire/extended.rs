//! BEP-10 extension protocol: handshake dict + ut_metadata (BEP-9) and
//! ut_pex (BEP-11) payloads
//!
//! On the wire an extended message is framed as a normal `Message::Extended`
//! with `ext_id == 0` identifying the extended-handshake; higher ids map to
//! per-peer extension message types negotiated via the handshake's `m` dict

use std::collections::HashMap;
use std::net::IpAddr;

use bytes::{BufMut, Bytes, BytesMut};

use super::super::bencode::{decode_all, encode_to_vec, Value};

pub const EXT_HANDSHAKE_ID: u8 = 0;

pub const EXT_NAME_UT_METADATA: &[u8] = b"ut_metadata";
pub const EXT_NAME_UT_PEX: &[u8] = b"ut_pex";

/// ut_metadata message types (BEP-9)
pub mod ut_metadata_type {
    pub const REQUEST: i64 = 0;
    pub const DATA: i64 = 1;
    pub const REJECT: i64 = 2;
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
#[derive(Debug)]
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

/// Encode a compact peer list as a bencoded ut_pex payload (just `added`)
pub fn build_ut_pex(v4: &[std::net::SocketAddrV4]) -> Bytes {
    let mut buf = BytesMut::with_capacity(v4.len() * 6);
    for addr in v4 {
        buf.put_slice(&addr.ip().octets());
        buf.put_u16(addr.port());
    }
    let dict = Value::Dict(vec![(b"added".to_vec(), Value::Bytes(buf.to_vec()))]);
    Bytes::from(encode_to_vec(&dict))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddrV4;

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
    fn ut_pex_round_trip() {
        let addrs = vec![
            SocketAddrV4::new("1.2.3.4".parse().unwrap(), 6881),
            SocketAddrV4::new("5.6.7.8".parse().unwrap(), 6882),
        ];
        let bytes = build_ut_pex(&addrs);
        let (v4, _) = parse_ut_pex(&bytes).unwrap();
        assert_eq!(v4.len(), 2);
        assert_eq!(v4[0].port(), 6881);
    }
}
