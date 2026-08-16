//! Public API types mirroring the librqbit surface; re-export / wrap submodule types so the engine uses one import path

use super::core::Id20;

/// Selector used by `Session::get`, `Session::pause`, `Session::delete`
#[derive(Debug, Clone, Copy)]
pub enum TorrentIdOrHash {
    Id(usize),
    Hash(Id20),
}
