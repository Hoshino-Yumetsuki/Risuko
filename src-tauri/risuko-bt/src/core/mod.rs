//! Primitive BitTorrent types: hashes, peer ids, magnet URIs, piece/chunk math,
//! and `.torrent` metainfo parsing

pub mod hash;
pub mod lengths;
pub mod magnet;
pub mod merkle;
pub mod metainfo;
pub mod peer_id;

pub use hash::{Id20, Id32};
pub use lengths::{ChunkInfo, Lengths, ValidPieceIndex, CHUNK_SIZE};
pub use magnet::Magnet;
pub use merkle::{supports_v2_wire, MerkleError, MerkleProofTable, PieceVerifier, VerifyError};
pub use metainfo::{
    parse_info_v2_from_bytes, FileDetails, MetaVersion, TorrentInfoHashes, TorrentMeta,
    TorrentMetaInfo, ValidatedTorrentMetaV1Info, ValidatedTorrentMetaV2Info,
};
pub use peer_id::{generate_peer_id, PEER_ID_PREFIX};
