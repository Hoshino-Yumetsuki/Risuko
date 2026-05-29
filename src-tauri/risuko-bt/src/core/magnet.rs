//! Magnet URI parser (BEP-9 / BEP-52 / BEP-53 subset)
//!
//! Supports v1 magnet links with `xt=urn:btih:<hex|base32>`, v2 magnet
//! links with `xt=urn:btmh:<multihash-hex>` (SHA-256 only), hybrid magnets
//! carrying both `xt` values, plus optional trackers (`tr=`), display
//! name (`dn=`), and `so=` (BEP-53) file-select indices

use std::str::FromStr;

use super::hash::{Id20, Id32};
use super::metainfo::TorrentInfoHashes;

#[derive(Debug, thiserror::Error)]
pub enum MagnetError {
    #[error("magnet: expected scheme magnet:, got {0:?}")]
    BadScheme(String),
    #[error("magnet: missing info-hash (xt=urn:btih:.. or xt=urn:btmh:..)")]
    MissingInfoHash,
    #[error("magnet: bad info-hash: {0}")]
    BadInfoHash(String),
    #[error("magnet: parse error: {0}")]
    Parse(String),
}

#[derive(Debug, Clone)]
pub struct Magnet {
    /// Wire infohash: v1 SHA-1 if present, else truncated SHA-256
    info_hash: Id20,
    /// Optional v1 SHA-1 (None for pure-v2 magnets)
    info_hash_v1: Option<Id20>,
    /// Optional v2 SHA-256 (None for pure-v1 magnets)
    info_hash_v2: Option<Id32>,
    pub trackers: Vec<String>,
    pub display_name: Option<String>,
    pub select_only: Option<Vec<usize>>,
}

impl Magnet {
    /// Wire infohash used for BEP-3 handshakes / trackers / v1 DHT
    /// For pure-v2 magnets this is the leading 20 bytes of the SHA-256 hash
    pub fn info_hash(&self) -> Id20 {
        self.info_hash
    }

    /// v1 (SHA-1) infohash if present
    pub fn info_hash_v1(&self) -> Option<Id20> {
        self.info_hash_v1
    }

    /// v2 (SHA-256) infohash if present
    pub fn info_hash_v2(&self) -> Option<Id32> {
        self.info_hash_v2
    }

    pub fn hashes(&self) -> TorrentInfoHashes {
        TorrentInfoHashes {
            v1: self.info_hash_v1,
            v2: self.info_hash_v2,
        }
    }

    pub fn as_id20(&self) -> Option<Id20> {
        Some(self.info_hash)
    }

    pub fn parse(input: &str) -> Result<Self, MagnetError> {
        let input = input.trim();
        if input.len() == 40 {
            if let Ok(id) = Id20::from_str(input) {
                return Ok(Self {
                    info_hash: id,
                    info_hash_v1: Some(id),
                    info_hash_v2: None,
                    trackers: vec![],
                    display_name: None,
                    select_only: None,
                });
            }
        }

        let url =
            url::Url::parse(input).map_err(|e| MagnetError::Parse(format!("invalid URL: {e}")))?;
        if url.scheme() != "magnet" {
            return Err(MagnetError::BadScheme(url.scheme().to_owned()));
        }

        let mut info_hash_v1: Option<Id20> = None;
        let mut info_hash_v2: Option<Id32> = None;
        let mut trackers = Vec::new();
        let mut display_name: Option<String> = None;
        let mut select_only: Option<Vec<usize>> = None;

        for (k, v) in url.query_pairs() {
            match &*k {
                "xt" => {
                    if let Some(rest) = v.strip_prefix("urn:btih:") {
                        let id = Id20::from_str(rest.trim())
                            .map_err(|_| MagnetError::BadInfoHash(rest.into()))?;
                        info_hash_v1 = Some(id);
                    } else if let Some(rest) = v.strip_prefix("urn:btmh:") {
                        // BEP 52: multihash hex string. Multihash framing:
                        // first byte = hash function code (0x12 = SHA-256),
                        // second byte = digest length (0x20 = 32 bytes),
                        // remaining bytes = digest
                        let raw = hex::decode(rest.trim())
                            .map_err(|_| MagnetError::BadInfoHash(rest.into()))?;
                        if raw.len() != 34 || raw[0] != 0x12 || raw[1] != 0x20 {
                            return Err(MagnetError::BadInfoHash(format!(
                                "btmh requires sha-256 multihash (0x12 0x20 + 32 bytes), got {} bytes",
                                raw.len()
                            )));
                        }
                        let id = Id32::from_slice(&raw[2..])
                            .map_err(|_| MagnetError::BadInfoHash(rest.into()))?;
                        info_hash_v2 = Some(id);
                    }
                }
                "tr" => trackers.push(v.into_owned()),
                "dn" => display_name = Some(v.into_owned()),
                "so" => {
                    // BEP-53 encoding: comma-separated indices or ranges a-b
                    let mut indices = Vec::new();
                    for part in v.split(',') {
                        let part = part.trim();
                        if part.is_empty() {
                            continue;
                        }
                        match part.split_once('-') {
                            Some((a, b)) => {
                                let a: usize = a.parse().map_err(|_| {
                                    MagnetError::Parse(format!("bad so range {part}"))
                                })?;
                                let b: usize = b.parse().map_err(|_| {
                                    MagnetError::Parse(format!("bad so range {part}"))
                                })?;
                                // Cap range expansion to prevent DoS from untrusted input
                                const MAX_SO_RANGE: usize = 100_000;
                                if b < a || b.saturating_sub(a) >= MAX_SO_RANGE {
                                    return Err(MagnetError::Parse(format!(
                                        "so range too large: {part}"
                                    )));
                                }
                                for i in a..=b {
                                    indices.push(i);
                                }
                            }
                            None => {
                                let i: usize = part
                                    .parse()
                                    .map_err(|_| MagnetError::Parse(format!("bad so {part}")))?;
                                indices.push(i);
                            }
                        }
                    }
                    if !indices.is_empty() {
                        select_only = Some(indices);
                    }
                }
                _ => {}
            }
        }

        let info_hash = match (info_hash_v1, info_hash_v2) {
            (Some(v1), _) => v1,
            (None, Some(v2)) => v2.truncate_to_id20(),
            (None, None) => return Err(MagnetError::MissingInfoHash),
        };

        Ok(Self {
            info_hash,
            info_hash_v1,
            info_hash_v2,
            trackers,
            display_name,
            select_only,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic_hex() {
        let m =
            Magnet::parse("magnet:?xt=urn:btih:cab507494d02ebb1178b38f2e9d7be299c86b862&dn=Hello")
                .unwrap();
        assert_eq!(
            m.info_hash.to_hex(),
            "cab507494d02ebb1178b38f2e9d7be299c86b862"
        );
        assert_eq!(m.display_name.as_deref(), Some("Hello"));
    }

    #[test]
    fn parse_base32() {
        // Base32 of 20 zero bytes
        let m = Magnet::parse("magnet:?xt=urn:btih:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA").unwrap();
        assert_eq!(m.info_hash.0, [0u8; 20]);
    }

    #[test]
    fn parse_trackers_and_so() {
        let m = Magnet::parse(
            "magnet:?xt=urn:btih:cab507494d02ebb1178b38f2e9d7be299c86b862&tr=http://a/announce&tr=udp://b:80&so=0,2-4",
        )
        .unwrap();
        assert_eq!(m.trackers.len(), 2);
        assert_eq!(m.select_only.as_deref(), Some(&[0usize, 2, 3, 4][..]));
    }

    #[test]
    fn parse_bare_hex() {
        let m = Magnet::parse("cab507494d02ebb1178b38f2e9d7be299c86b862").unwrap();
        assert_eq!(m.trackers.len(), 0);
    }

    #[test]
    fn reject_missing_xt() {
        assert!(matches!(
            Magnet::parse("magnet:?dn=nothing"),
            Err(MagnetError::MissingInfoHash),
        ));
    }

    #[test]
    fn parse_v2_btmh() {
        // 0x12 0x20 + 32 zero bytes
        let mut multihash = vec![0x12u8, 0x20];
        multihash.extend_from_slice(&[0u8; 32]);
        let hex = hex::encode(multihash);
        let m = Magnet::parse(&format!("magnet:?xt=urn:btmh:{hex}")).unwrap();
        assert!(m.info_hash_v1().is_none());
        let v2 = m.info_hash_v2().unwrap();
        assert_eq!(v2.0, [0u8; 32]);
        // wire infohash for pure-v2 = truncated v2
        assert_eq!(m.info_hash().0, [0u8; 20]);
    }

    #[test]
    fn parse_hybrid_magnet() {
        let mut multihash = vec![0x12u8, 0x20];
        multihash.extend_from_slice(&[0xabu8; 32]);
        let v2hex = hex::encode(multihash);
        let v1hex = "cab507494d02ebb1178b38f2e9d7be299c86b862";
        let m = Magnet::parse(&format!("magnet:?xt=urn:btih:{v1hex}&xt=urn:btmh:{v2hex}")).unwrap();
        let v1 = m.info_hash_v1().unwrap();
        assert_eq!(v1.to_hex(), v1hex);
        assert_eq!(m.info_hash_v2().unwrap().0, [0xabu8; 32]);
        // Hybrid wire infohash prefers v1
        assert_eq!(m.info_hash(), v1);
    }

    #[test]
    fn reject_btmh_wrong_multihash_code() {
        // 0x11 = SHA-1 multihash code; we only accept 0x12 (SHA-256)
        let mut bad = vec![0x11u8, 0x14]; // sha1, 20 bytes
        bad.extend_from_slice(&[0u8; 20]);
        let hex = hex::encode(bad);
        assert!(matches!(
            Magnet::parse(&format!("magnet:?xt=urn:btmh:{hex}")),
            Err(MagnetError::BadInfoHash(_)),
        ));
    }
}
