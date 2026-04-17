//! Magnet URI parser (BEP-9 / BEP-53 subset)
//!
//! We support v1 magnet links with `xt=urn:btih:<hex|base32>`, optional
//! trackers (`tr=`), display name (`dn=`), and `so=` (BEP-53) file-select
//! indices. v2 (btmh) is recognised but not currently downloadable

use std::str::FromStr;

use super::hash::Id20;

#[derive(Debug, thiserror::Error)]
pub enum MagnetError {
    #[error("magnet: expected scheme magnet:, got {0:?}")]
    BadScheme(String),
    #[error("magnet: missing info-hash (xt=urn:btih:..)")]
    MissingInfoHash,
    #[error("magnet: bad info-hash: {0}")]
    BadInfoHash(String),
    #[error("magnet: parse error: {0}")]
    Parse(String),
}

#[derive(Debug, Clone)]
pub struct Magnet {
    info_hash: Id20,
    pub trackers: Vec<String>,
    pub display_name: Option<String>,
    pub select_only: Option<Vec<usize>>,
}

impl Magnet {
    pub fn info_hash(&self) -> Id20 {
        self.info_hash
    }

    /// Kept for API compatibility with librqbit shims
    pub fn as_id20(&self) -> Option<Id20> {
        Some(self.info_hash)
    }

    pub fn parse(input: &str) -> Result<Self, MagnetError> {
        // Accept a bare 40-char hex hash as a shortcut — useful for CLI use
        // and matches librqbit's behaviour
        let input = input.trim();
        if input.len() == 40 {
            if let Ok(id) = Id20::from_str(input) {
                return Ok(Self {
                    info_hash: id,
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

        let mut info_hash: Option<Id20> = None;
        let mut trackers = Vec::new();
        let mut display_name: Option<String> = None;
        let mut select_only: Option<Vec<usize>> = None;

        for (k, v) in url.query_pairs() {
            match &*k {
                "xt" => {
                    if let Some(rest) = v.strip_prefix("urn:btih:") {
                        let id = Id20::from_str(rest.trim())
                            .map_err(|_| MagnetError::BadInfoHash(rest.into()))?;
                        info_hash = Some(id);
                    }
                    // urn:btmh (v2) is ignored for now
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

        let info_hash = info_hash.ok_or(MagnetError::MissingInfoHash)?;
        Ok(Self {
            info_hash,
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
}
