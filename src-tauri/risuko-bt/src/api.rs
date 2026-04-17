//! Public API types mirroring the librqbit surface that `engine::torrent.rs`
//! consumes. These re-export / wrap types from submodules so the rest of the
//! engine can use one import path

use super::core::Id20;

/// Selector used by `Session::get`, `Session::pause`, `Session::delete`
#[derive(Debug, Clone, Copy)]
pub enum TorrentIdOrHash {
    Id(usize),
    Hash(Id20),
}
