//! BitTorrent peer state tracked per connection

use std::time::Instant;

use bytes::Bytes;

#[derive(Debug, Clone, Copy, Default)]
pub struct PeerFlags {
    /// We are choking the remote
    pub am_choking: bool,
    /// We have declared interest in the remote's pieces
    pub am_interested: bool,
    /// The remote is choking us
    pub peer_choking: bool,
    /// The remote has declared interest in our pieces
    pub peer_interested: bool,
}

impl PeerFlags {
    pub fn new() -> Self {
        // BEP-3 default: both sides start choked & not interested.
        Self {
            am_choking: true,
            am_interested: false,
            peer_choking: true,
            peer_interested: false,
        }
    }
}

#[derive(Debug)]
pub struct PeerState {
    pub flags: PeerFlags,
    /// Peer's advertised bitfield (MSB-first). Updated on `Bitfield` and
    /// individual `Have` messages
    pub bitfield: Vec<u8>,
    /// Monotonic instant of the last message we received. Used to cull
    /// idle/unresponsive peers
    pub last_activity: Instant,
    /// Peer-supplied ut_metadata id (post ext handshake), if any
    pub ut_metadata_id: Option<u8>,
    pub ut_pex_id: Option<u8>,
    /// Peer-advertised metadata size from ext handshake
    pub metadata_size: Option<u64>,
    pub client: Option<String>,
    pub bytes_up: u64,
    pub bytes_down: u64,
    /// Last sent request queue — used by the torrent to issue cancels
    pub outstanding_requests: Vec<OutstandingRequest>,
}

#[derive(Debug, Clone)]
pub struct OutstandingRequest {
    pub index: u32,
    pub begin: u32,
    pub length: u32,
    pub sent_at: Instant,
}

impl PeerState {
    pub fn new(bitfield_bytes: usize) -> Self {
        Self {
            flags: PeerFlags::new(),
            bitfield: vec![0u8; bitfield_bytes],
            last_activity: Instant::now(),
            ut_metadata_id: None,
            ut_pex_id: None,
            metadata_size: None,
            client: None,
            bytes_up: 0,
            bytes_down: 0,
            outstanding_requests: Vec::new(),
        }
    }

    pub fn merge_bitfield(&mut self, bf: &Bytes) {
        let n = bf.len().min(self.bitfield.len());
        self.bitfield[..n].copy_from_slice(&bf[..n]);
    }

    pub fn set_have(&mut self, piece: u32) {
        let byte = (piece / 8) as usize;
        let bit = 7 - (piece % 8) as u8;
        if byte < self.bitfield.len() {
            self.bitfield[byte] |= 1 << bit;
        }
    }

    pub fn has_piece(&self, piece: u32) -> bool {
        let byte = (piece / 8) as usize;
        let bit = 7 - (piece % 8) as u8;
        self.bitfield
            .get(byte)
            .map(|b| b & (1 << bit) != 0)
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitfield_updates() {
        let mut s = PeerState::new(2);
        s.set_have(0);
        s.set_have(9);
        assert!(s.has_piece(0));
        assert!(s.has_piece(9));
        assert!(!s.has_piece(1));
    }
}
