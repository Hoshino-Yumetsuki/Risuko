//! `.torrent` metainfo parsing (BEP-3)
//!
//! Produces [`TorrentMeta`] with the raw `info` dict bytes preserved so the
//! info-hash can be recomputed. [`ValidatedTorrentMetaV1Info`] wraps a parsed
//! info dict with an enumerator over per-file details, matching the API shape
//! that `engine::torrent` consumes from librqbit

use std::path::PathBuf;

use super::super::bencode::{decode_dict_field_raw, Value};
use super::hash::{sha1, Id20};

#[derive(Debug, thiserror::Error)]
pub enum MetaError {
    #[error("bencode: {0}")]
    Bencode(#[from] super::super::bencode::Error),
    #[error("metainfo: missing `info` dict")]
    MissingInfo,
    #[error("metainfo: malformed info dict: {0}")]
    BadInfo(&'static str),
    #[error("metainfo: invalid UTF-8 in {field}")]
    BadUtf8 { field: &'static str },
    #[error("metainfo: torrent has zero length")]
    ZeroLength,
    #[error("metainfo: pieces field is not a multiple of 20 bytes")]
    BadPieces,
}

/// Raw top-level `.torrent` metadata
#[derive(Debug, Clone)]
pub struct TorrentMeta {
    pub info: ValidatedTorrentMetaV1Info,
    /// Primary announce URL, if any
    pub announce: Option<String>,
    /// `announce-list` tiers (BEP-12)
    pub announce_list: Vec<Vec<String>>,
    pub comment: Option<String>,
    pub created_by: Option<String>,
    pub creation_date: Option<i64>,
    pub encoding: Option<String>,
    /// Info-hash computed over the raw bytes of the `info` dict
    pub info_hash: Id20,
}

/// Parsed and validated `info` dictionary
#[derive(Debug, Clone)]
pub struct ValidatedTorrentMetaV1Info {
    pub name: String,
    pub piece_length: u32,
    /// SHA-1 hash per piece, in order. Length == `piece_count * 20`
    pub pieces: Vec<u8>,
    pub private: bool,
    pub files: Vec<TorrentMetaInfo>,
    /// True if the torrent described a single file (no `files` list)
    pub single_file_mode: bool,
}

/// A single file entry as viewed by the rest of the engine
#[derive(Debug, Clone)]
pub struct TorrentMetaInfo {
    /// Components relative to the torrent root (never absolute, never `..`)
    pub path: Vec<String>,
    pub length: u64,
}

/// Aggregated file view returned by [`ValidatedTorrentMetaV1Info::iter_file_details`]
#[derive(Debug, Clone)]
pub struct FileDetails {
    /// Joined path, suitable for display. Slashes are used regardless of OS
    pub filename: String,
    pub len: u64,
}

impl ValidatedTorrentMetaV1Info {
    pub fn iter_file_details(&self) -> impl Iterator<Item = FileDetails> + '_ {
        self.files.iter().map(|f| FileDetails {
            filename: f.path.join("/"),
            len: f.length,
        })
    }

    /// Build a [`PathBuf`] rooted at `base` for a file entry
    pub fn file_path(&self, base: &std::path::Path, idx: usize) -> PathBuf {
        let mut p = base.to_path_buf();
        if !self.single_file_mode {
            p.push(&self.name);
        }
        for c in &self.files[idx].path {
            p.push(c);
        }
        p
    }

    pub fn piece_count(&self) -> u32 {
        (self.pieces.len() / 20) as u32
    }

    pub fn piece_hash(&self, idx: u32) -> Option<Id20> {
        let start = idx as usize * 20;
        let slice = self.pieces.get(start..start + 20)?;
        Id20::from_slice(slice).ok()
    }

    pub fn total_length(&self) -> u64 {
        self.files.iter().map(|f| f.length).sum()
    }
}

/// Parse a `.torrent` blob
pub fn parse_torrent(bytes: &[u8]) -> Result<TorrentMeta, MetaError> {
    let value = super::super::bencode::decode_all(bytes)?;
    let dict = value
        .as_dict()
        .ok_or(MetaError::BadInfo("top-level not dict"))?;

    let announce = get_str(dict, b"announce");
    let announce_list = dict
        .iter()
        .find(|(k, _)| k == b"announce-list")
        .and_then(|(_, v)| v.as_list())
        .map(|tiers| {
            tiers
                .iter()
                .filter_map(|tier| tier.as_list())
                .map(|urls| {
                    urls.iter()
                        .filter_map(|u| u.as_str().map(String::from))
                        .collect()
                })
                .collect::<Vec<Vec<String>>>()
        })
        .unwrap_or_default();
    let comment = get_str(dict, b"comment");
    let created_by = get_str(dict, b"created by");
    let encoding = get_str(dict, b"encoding");
    let creation_date = dict
        .iter()
        .find(|(k, _)| k == b"creation date")
        .and_then(|(_, v)| v.as_int());

    // Recover raw bytes of the `info` field to compute the info-hash
    let (info_value, info_raw) =
        decode_dict_field_raw(bytes, b"info")?.ok_or(MetaError::MissingInfo)?;
    let info_hash = sha1(info_raw);

    let info = validate_info(&info_value)?;
    Ok(TorrentMeta {
        info,
        announce,
        announce_list,
        comment,
        created_by,
        creation_date,
        encoding,
        info_hash,
    })
}

fn get_str(dict: &[(Vec<u8>, Value)], key: &[u8]) -> Option<String> {
    dict.iter()
        .find(|(k, _)| k == key)
        .and_then(|(_, v)| v.as_str().map(String::from))
}

fn validate_info(value: &Value) -> Result<ValidatedTorrentMetaV1Info, MetaError> {
    let info = value.as_dict().ok_or(MetaError::BadInfo("info not dict"))?;

    let piece_length = info
        .iter()
        .find(|(k, _)| k == b"piece length")
        .and_then(|(_, v)| v.as_int())
        .ok_or(MetaError::BadInfo("piece length missing"))?;
    if !(0..=u32::MAX as i64).contains(&piece_length) || piece_length == 0 {
        return Err(MetaError::BadInfo("piece length out of range"));
    }
    let piece_length = piece_length as u32;

    let pieces_bytes = info
        .iter()
        .find(|(k, _)| k == b"pieces")
        .and_then(|(_, v)| v.as_bytes())
        .ok_or(MetaError::BadInfo("pieces missing"))?;
    if pieces_bytes.is_empty() || pieces_bytes.len() % 20 != 0 {
        return Err(MetaError::BadPieces);
    }

    let name = info
        .iter()
        .find(|(k, _)| k == b"name")
        .and_then(|(_, v)| v.as_str())
        .ok_or(MetaError::BadUtf8 { field: "name" })?
        .to_string();

    let private = info
        .iter()
        .find(|(k, _)| k == b"private")
        .and_then(|(_, v)| v.as_int())
        .is_some_and(|n| n != 0);

    let (files, single_file_mode) = if let Some(list) = info
        .iter()
        .find(|(k, _)| k == b"files")
        .and_then(|(_, v)| v.as_list())
    {
        let mut files = Vec::with_capacity(list.len());
        for entry in list {
            let entry = entry
                .as_dict()
                .ok_or(MetaError::BadInfo("files entry not dict"))?;
            let length = entry
                .iter()
                .find(|(k, _)| k == b"length")
                .and_then(|(_, v)| v.as_int())
                .ok_or(MetaError::BadInfo("file length missing"))?;
            if length < 0 {
                return Err(MetaError::BadInfo("file length negative"));
            }
            let path_list = entry
                .iter()
                .find(|(k, _)| k == b"path")
                .and_then(|(_, v)| v.as_list())
                .ok_or(MetaError::BadInfo("file path missing"))?;
            let mut path_components = Vec::with_capacity(path_list.len());
            for c in path_list {
                let s = c
                    .as_str()
                    .ok_or(MetaError::BadUtf8 { field: "file path" })?;
                if s.is_empty() || s == ".." || s.contains('/') || s.contains('\\') {
                    return Err(MetaError::BadInfo("file path component unsafe"));
                }
                path_components.push(s.to_string());
            }
            files.push(TorrentMetaInfo {
                path: path_components,
                length: length as u64,
            });
        }
        (files, false)
    } else {
        let length = info
            .iter()
            .find(|(k, _)| k == b"length")
            .and_then(|(_, v)| v.as_int())
            .ok_or(MetaError::BadInfo("single-file length missing"))?;
        if length <= 0 {
            return Err(MetaError::ZeroLength);
        }
        (
            vec![TorrentMetaInfo {
                path: vec![name.clone()],
                length: length as u64,
            }],
            true,
        )
    };

    if files.iter().map(|f| f.length).sum::<u64>() == 0 {
        return Err(MetaError::ZeroLength);
    }

    Ok(ValidatedTorrentMetaV1Info {
        name,
        piece_length,
        pieces: pieces_bytes.to_vec(),
        private,
        files,
        single_file_mode,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synth_single_file(name: &str, piece_len: u32, data_len: u64) -> Vec<u8> {
        // Craft a minimal valid single-file .torrent (announce-less). Piece
        // hashes are zeroed; sufficient for parser tests
        let piece_count = data_len.div_ceil(piece_len as u64);
        let pieces = vec![0u8; (piece_count * 20) as usize];
        // Build info dict via the encoder to guarantee canonical output
        let info = Value::Dict(vec![
            (b"length".to_vec(), Value::Int(data_len as i64)),
            (b"name".to_vec(), Value::Bytes(name.as_bytes().to_vec())),
            (b"piece length".to_vec(), Value::Int(piece_len as i64)),
            (b"pieces".to_vec(), Value::Bytes(pieces)),
        ]);
        let top = Value::Dict(vec![
            (
                b"announce".to_vec(),
                Value::Bytes(b"http://tracker/announce".to_vec()),
            ),
            (b"info".to_vec(), info),
        ]);
        super::super::super::bencode::encode_to_vec(&top)
    }

    #[test]
    fn parse_single_file() {
        let bytes = synth_single_file("hello.bin", 16 * 1024, 100_000);
        let meta = parse_torrent(&bytes).unwrap();
        assert_eq!(meta.info.name, "hello.bin");
        assert_eq!(meta.info.piece_length, 16 * 1024);
        assert_eq!(meta.info.files.len(), 1);
        assert_eq!(meta.info.files[0].length, 100_000);
        assert!(meta.info.single_file_mode);
        assert_eq!(meta.announce.as_deref(), Some("http://tracker/announce"));
        // info-hash must be stable regardless of re-encode order (dict keys
        // are sorted by the encoder already)
        assert_ne!(meta.info_hash.to_hex(), "0".repeat(40));
    }

    #[test]
    fn parse_multi_file_rejects_unsafe_path() {
        let info = Value::Dict(vec![
            (
                b"files".to_vec(),
                Value::List(vec![Value::Dict(vec![
                    (b"length".to_vec(), Value::Int(10)),
                    (
                        b"path".to_vec(),
                        Value::List(vec![
                            Value::Bytes(b"..".to_vec()),
                            Value::Bytes(b"escape".to_vec()),
                        ]),
                    ),
                ])]),
            ),
            (b"name".to_vec(), Value::Bytes(b"multi".to_vec())),
            (b"piece length".to_vec(), Value::Int(16 * 1024)),
            (b"pieces".to_vec(), Value::Bytes(vec![0; 20])),
        ]);
        let top = Value::Dict(vec![(b"info".to_vec(), info)]);
        let bytes = super::super::super::bencode::encode_to_vec(&top);
        assert!(matches!(parse_torrent(&bytes), Err(MetaError::BadInfo(_))));
    }

    #[test]
    fn piece_hash_extractable() {
        let bytes = synth_single_file("a", 16 * 1024, 100);
        let meta = parse_torrent(&bytes).unwrap();
        let h = meta.info.piece_hash(0).unwrap();
        assert_eq!(h.0, [0u8; 20]);
    }
}
