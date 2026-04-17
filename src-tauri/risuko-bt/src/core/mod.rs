//! Primitive BitTorrent types: hashes, peer ids, magnet URIs, piece/chunk math,
//! and `.torrent` metainfo parsing

pub mod hash;
pub mod lengths;
pub mod magnet;
pub mod metainfo;
pub mod peer_id;

pub use hash::Id20;
pub use lengths::{ChunkInfo, Lengths, PieceInfo, ValidPieceIndex, CHUNK_SIZE};
pub use magnet::Magnet;
pub use metainfo::{FileDetails, TorrentMeta, TorrentMetaInfo, ValidatedTorrentMetaV1Info};
pub use peer_id::{generate_peer_id, PEER_ID_PREFIX};
