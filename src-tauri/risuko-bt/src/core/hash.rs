//! Fixed-length byte ids — primarily SHA-1 (20 bytes)

use std::fmt;
use std::str::FromStr;

/// A 20-byte id used for info-hashes, peer ids and DHT node ids
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Id20(pub [u8; 20]);

impl Id20 {
    pub const LEN: usize = 20;

    pub fn new(bytes: [u8; 20]) -> Self {
        Self(bytes)
    }

    pub fn from_slice(b: &[u8]) -> Result<Self, HashParseError> {
        if b.len() != 20 {
            return Err(HashParseError::BadLength(b.len()));
        }
        let mut v = [0u8; 20];
        v.copy_from_slice(b);
        Ok(Self(v))
    }

    pub fn as_bytes(&self) -> &[u8; 20] {
        &self.0
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    /// Alias of `to_hex`, kept to match the librqbit API shape that
    /// `engine::torrent.rs` consumes
    pub fn as_string(&self) -> String {
        self.to_hex()
    }

    /// XOR distance used by Kademlia routing
    pub fn distance(&self, other: &Self) -> Self {
        let mut d = [0u8; 20];
        for i in 0..20 {
            d[i] = self.0[i] ^ other.0[i];
        }
        Self(d)
    }

    /// MSB-first bit access (bit 0 is the high bit of byte 0)
    pub fn get_bit(&self, bit: u8) -> bool {
        if bit >= 160 {
            return false;
        }
        let byte = self.0[(bit / 8) as usize];
        byte & (1 << (7 - bit % 8)) != 0
    }
}

impl fmt::Debug for Id20 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Id20({})", self.to_hex())
    }
}

impl fmt::Display for Id20 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HashParseError {
    #[error("expected 20 bytes, got {0}")]
    BadLength(usize),
    #[error("hex decode failed")]
    BadHex,
    #[error("base32 decode failed")]
    BadBase32,
}

impl FromStr for Id20 {
    type Err = HashParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.len() {
            40 => {
                let raw = hex::decode(s).map_err(|_| HashParseError::BadHex)?;
                Self::from_slice(&raw)
            }
            32 => {
                // RFC 4648 base32 with padding stripped
                let decoded = base32_decode_upper(s.as_bytes()).ok_or(HashParseError::BadBase32)?;
                Self::from_slice(&decoded)
            }
            n => Err(HashParseError::BadLength(n)),
        }
    }
}

/// Decode RFC 4648 base32 (uppercase, optional padding). Returns None on
/// invalid input. Used for BTIH magnet links where the hash may be base32
fn base32_decode_upper(input: &[u8]) -> Option<Vec<u8>> {
    const ALPHA: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let input: Vec<u8> = input
        .iter()
        .copied()
        .filter(|b| *b != b'=')
        .map(|b| b.to_ascii_uppercase())
        .collect();

    let mut out = Vec::with_capacity(input.len() * 5 / 8);
    let mut buf: u32 = 0;
    let mut bits: u32 = 0;
    for c in input {
        let v = ALPHA.iter().position(|&a| a == c)? as u32;
        buf = (buf << 5) | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xFF) as u8);
        }
    }
    Some(out)
}

/// Convenience: SHA-1 hash of `data` as [`Id20`]
pub fn sha1(data: &[u8]) -> Id20 {
    use sha1::Digest;
    let mut h = sha1::Sha1::new();
    h.update(data);
    let out = h.finalize();
    Id20(out.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip() {
        let id = Id20([0xabu8; 20]);
        let hex = id.to_hex();
        assert_eq!(hex.len(), 40);
        let parsed: Id20 = hex.parse().unwrap();
        assert_eq!(parsed, id);
    }

    #[test]
    fn base32_parse_known() {
        // Well-known magnet BTIH base32 example: 20 zero bytes
        let encoded = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"; // 32 chars
        let parsed: Id20 = encoded.parse().unwrap();
        assert_eq!(parsed.0, [0u8; 20]);
    }

    #[test]
    fn xor_distance() {
        let a = Id20([0xffu8; 20]);
        let b = Id20([0x0fu8; 20]);
        let d = a.distance(&b);
        assert_eq!(d.0, [0xf0u8; 20]);
    }

    #[test]
    fn bit_access() {
        let mut raw = [0u8; 20];
        raw[0] = 0b1000_0000;
        let id = Id20(raw);
        assert!(id.get_bit(0));
        assert!(!id.get_bit(1));
    }

    #[test]
    fn sha1_matches_known() {
        let empty = sha1(b"");
        assert_eq!(empty.to_hex(), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
    }
}
