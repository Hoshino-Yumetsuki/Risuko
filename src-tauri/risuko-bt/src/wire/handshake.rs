//! BitTorrent handshake (BEP-3): 68 bytes total.
//!
//! ```text
//!   1   byte   length of protocol string (always 19)
//!   19  bytes  "BitTorrent protocol"
//!   8   bytes  reserved (BEP-10 bit: ext protocol; BEP-5 bit: DHT; …)
//!   20  bytes  info-hash
//!   20  bytes  peer-id
//! ```

use super::super::core::Id20;

pub const PROTOCOL: &[u8; 19] = b"BitTorrent protocol";
pub const HANDSHAKE_LEN: usize = 1 + 19 + 8 + 20 + 20;

/// Reserved bit flags we care about. Byte indices are 0-based from the start
/// of the 8-byte reserved area
pub mod reserved {
    /// Bit 20 (byte 5, bit 0x10) — BEP-10 Extension Protocol
    pub const EXT_PROTOCOL: (usize, u8) = (5, 0x10);
    /// Last bit — BEP-5 DHT
    pub const DHT: (usize, u8) = (7, 0x01);
    /// Bit 2 of byte 7 — BEP-6 Fast extension
    pub const FAST: (usize, u8) = (7, 0x04);
    /// Bit 3 of byte 7 (0x08) — BEP 52 BitTorrent v2 capable
    /// Matches libtorrent's choice for maximum interop. The bit position
    /// is not yet ratified; flip this single constant if it shifts
    pub const V2: (usize, u8) = (7, 0x08);
}

#[derive(Debug, thiserror::Error)]
pub enum HandshakeError {
    #[error("bad protocol string length {0}")]
    BadPstrLen(u8),
    #[error("bad protocol string")]
    BadProtocol,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Handshake {
    pub reserved: [u8; 8],
    pub info_hash: Id20,
    pub peer_id: Id20,
}

impl Handshake {
    pub fn new_with_v2(info_hash: Id20, peer_id: Id20, advertise_v2: bool) -> Self {
        // We always advertise the BEP-10 extension-protocol bit. The BEP-52 v2 bit
        // is *only* set when the caller is explicitly connecting on a v2 info-hash
        // (pure-v2 swarm)—empirically, blanket-setting it on v1 / hybrid connections
        // causes some peers (notably Thunder / Xunlei-style clients common in CN swarms)
        // to close the socket right after the BT handshake exchange. We never set the
        // BEP-5 DHT bit because we do not act on inbound `Port` messages from the
        // BT layer
        let mut reserved = [0u8; 8];
        let (b, m) = reserved::EXT_PROTOCOL;
        reserved[b] |= m;
        if advertise_v2 {
            let (b, m) = reserved::V2;
            reserved[b] |= m;
        }
        Self {
            reserved,
            info_hash,
            peer_id,
        }
    }

    pub fn has_ext_protocol(&self) -> bool {
        let (b, m) = reserved::EXT_PROTOCOL;
        self.reserved[b] & m != 0
    }

    /// True if the peer advertises BEP 52 (BitTorrent v2) capability
    pub fn has_v2(&self) -> bool {
        let (b, m) = reserved::V2;
        self.reserved[b] & m != 0
    }

    pub fn to_bytes(&self) -> [u8; HANDSHAKE_LEN] {
        let mut out = [0u8; HANDSHAKE_LEN];
        out[0] = 19;
        out[1..20].copy_from_slice(PROTOCOL);
        out[20..28].copy_from_slice(&self.reserved);
        out[28..48].copy_from_slice(self.info_hash.as_bytes());
        out[48..68].copy_from_slice(self.peer_id.as_bytes());
        out
    }

    pub fn parse(buf: &[u8; HANDSHAKE_LEN]) -> Result<Self, HandshakeError> {
        if buf[0] != 19 {
            return Err(HandshakeError::BadPstrLen(buf[0]));
        }
        if &buf[1..20] != PROTOCOL {
            return Err(HandshakeError::BadProtocol);
        }
        let mut reserved = [0u8; 8];
        reserved.copy_from_slice(&buf[20..28]);
        let info_hash = Id20::from_slice(&buf[28..48]).unwrap();
        let peer_id = Id20::from_slice(&buf[48..68]).unwrap();
        Ok(Self {
            reserved,
            info_hash,
            peer_id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_advertises_ext_and_optionally_v2() {
        let hs = Handshake::new_with_v2(Id20([0xaau8; 20]), Id20([0xbbu8; 20]), true);
        let bytes = hs.to_bytes();
        let parsed = Handshake::parse(&bytes).unwrap();
        assert_eq!(hs, parsed);
        assert!(parsed.has_ext_protocol());
        // `advertise_v2 = true` carries the v2 capability bit
        assert!(parsed.has_v2());
    }

    #[test]
    fn v2_reserved_bit_is_caller_controlled() {
        let hs_off = Handshake::new_with_v2(Id20([0xaau8; 20]), Id20([0xbbu8; 20]), false);
        let hs_on = Handshake::new_with_v2(Id20([0xaau8; 20]), Id20([0xbbu8; 20]), true);
        assert!(hs_off.has_ext_protocol());
        assert!(!hs_off.has_v2());
        assert!(hs_on.has_v2());
    }

    #[test]
    fn rejects_bad_protocol() {
        let mut bytes = [0u8; HANDSHAKE_LEN];
        bytes[0] = 19;
        bytes[1..20].copy_from_slice(b"NotBitTorrentProto!");
        assert!(matches!(
            Handshake::parse(&bytes),
            Err(HandshakeError::BadProtocol)
        ));
    }

    #[test]
    fn rejects_bad_pstrlen() {
        let mut bytes = [0u8; HANDSHAKE_LEN];
        bytes[0] = 42;
        assert!(matches!(
            Handshake::parse(&bytes),
            Err(HandshakeError::BadPstrLen(42))
        ));
    }
}
