//! Per-chunk state machine: schedules outstanding chunk requests and
//! coordinates endgame (duplicate requests to multiple peers)

use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::super::core::lengths::{ChunkInfo, Lengths, ValidPieceIndex};

/// A chunk that was reclaimed from a peer whose request exceeded the
/// request timeout. Returned by [`ChunkTracker::reclaim_stale`] so the
/// torrent loop can also clear the piece's in-flight flag (so it becomes
/// requestable again) and optionally free the peer's outstanding slot
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReclaimedChunk {
    pub piece: u32,
    pub chunk_index: u32,
    /// Byte offset within the piece; matches `Message::Request.begin`
    pub begin: u32,
    pub peer: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkState {
    Missing,
    Requested { peer: u32, since: Instant },
    Received,
}

#[derive(Debug, Clone, Copy)]
pub struct ChunkRequest {
    pub info: ChunkInfo,
    /// State that was overwritten when this request was issued. Used by
    /// `unrequest_chunk` to roll back without losing another peer's
    /// outstanding `Requested` slot in endgame mode
    pub prior_state: ChunkState,
}

#[derive(Debug)]
pub struct ChunkTracker {
    lengths: Lengths,
    // chunk_index -> state. We allocate lazily per piece to avoid upfront
    // memory cost on very large torrents. The `pieces` map keys on piece
    // index; the value is a dense Vec<ChunkState> sized to the piece's chunks
    pieces: HashMap<u32, Vec<ChunkState>>,
    /// Global switch: when the set of missing chunks is small we issue
    /// duplicate requests to multiple peers to finish faster
    endgame: bool,
}

impl ChunkTracker {
    pub fn new(lengths: Lengths) -> Self {
        Self {
            lengths,
            pieces: HashMap::new(),
            endgame: false,
        }
    }

    pub fn lengths(&self) -> &Lengths {
        &self.lengths
    }

    pub fn set_endgame(&mut self, on: bool) {
        self.endgame = on;
    }

    pub fn endgame(&self) -> bool {
        self.endgame
    }

    /// Return the next chunk to request from a peer for `piece`, if any. In
    /// endgame mode, `Requested` chunks owned by other peers are also
    /// returned (duplicated). In endgame, scanning starts at a `peer`-derived
    /// offset so different peers spread duplication across the piece instead
    /// of all re-requesting chunk 0 first — mirrors aria2's `std::shuffle`
    /// in `createRequestMessagesOnEndGame`. In normal mode the scan stays
    /// sequential so a single peer's chunks land in offset order, which
    /// helps disk write coalescing
    pub fn next_chunk(&mut self, piece: ValidPieceIndex, peer: u32) -> Option<ChunkRequest> {
        let endgame = self.endgame;
        let states = self.states_for(piece);
        let len = states.len();
        if len == 0 {
            return None;
        }
        let start = if endgame {
            // Stable per-(peer, piece) offset; cheap mixing constant from
            // Knuth's multiplicative hash. No RNG needed
            (peer as usize)
                .wrapping_mul(2_654_435_761)
                .wrapping_add(piece.get() as usize)
                % len
        } else {
            0
        };
        let mut candidate: Option<usize> = None;
        let mut missing: Option<usize> = None;
        for k in 0..len {
            let i = (start + k) % len;
            match states[i] {
                ChunkState::Missing => {
                    missing = Some(i);
                    break;
                }
                ChunkState::Requested { peer: p, .. }
                    if endgame && p != peer && candidate.is_none() =>
                {
                    candidate = Some(i);
                }
                _ => {}
            }
        }
        let i = missing.or(candidate)?;
        let info = self
            .lengths
            .chunks_of(piece)
            .nth(i)
            .expect("chunk index within piece");
        let states = self.pieces.get_mut(&piece.get()).unwrap();
        let prior_state = states[i];
        states[i] = ChunkState::Requested {
            peer,
            since: Instant::now(),
        };
        Some(ChunkRequest { info, prior_state })
    }

    /// Mark a received chunk. Returns true if the full piece is now complete
    pub fn mark_received(&mut self, info: ChunkInfo) -> bool {
        let states = self.states_for(info.piece_index);
        let idx = info.chunk_index as usize;
        if idx < states.len() {
            let states = self.pieces.get_mut(&info.piece_index.get()).unwrap();
            states[idx] = ChunkState::Received;
            states.iter().all(|s| matches!(s, ChunkState::Received))
        } else {
            false
        }
    }

    /// Re-mark outstanding chunks as missing — used when a peer disconnects
    /// Returns piece indices that now have freed chunks
    pub fn release_peer(&mut self, peer: u32) -> Vec<u32> {
        let mut affected = Vec::new();
        for (&piece_idx, states) in self.pieces.iter_mut() {
            let mut freed = false;
            for s in states.iter_mut() {
                if let ChunkState::Requested { peer: p, .. } = *s {
                    if p == peer {
                        *s = ChunkState::Missing;
                        freed = true;
                    }
                }
            }
            if freed {
                affected.push(piece_idx);
            }
        }
        affected
    }

    /// Reset chunk state for a piece — called on SHA-1 mismatch.=
    pub fn reset_piece(&mut self, piece: ValidPieceIndex) {
        if let Some(states) = self.pieces.get_mut(&piece.get()) {
            for s in states {
                *s = ChunkState::Missing;
            }
        }
    }

    /// Roll back a single chunk request (e.g. peer's send channel was full).
    /// Restores the state that `next_chunk` overwrote, so an in-flight
    /// `Requested` slot owned by a different peer (endgame duplication) is
    /// preserved. Without this, a try_send failure would either strand the
    /// chunk in `Requested` forever or wipe another peer's outstanding
    /// request
    pub fn unrequest_chunk(
        &mut self,
        piece: ValidPieceIndex,
        chunk_index: u32,
        prior_state: ChunkState,
    ) {
        if let Some(states) = self.pieces.get_mut(&piece.get()) {
            if let Some(s) = states.get_mut(chunk_index as usize) {
                if matches!(s, ChunkState::Requested { .. }) {
                    *s = prior_state;
                }
            }
        }
    }

    /// Number of chunks that are not yet `Received` across pieces we've
    /// touched. Used to decide whether to enable endgame mode (where
    /// outstanding chunks are duplicated to multiple peers to drain the
    /// last few stragglers). Cheap because we only look at pieces with at
    /// least one chunk requested
    pub fn pending_chunks(&self) -> usize {
        self.pieces
            .values()
            .map(|states| {
                states
                    .iter()
                    .filter(|s| !matches!(s, ChunkState::Received))
                    .count()
            })
            .sum()
    }

    /// Scan for chunks that have been in `Requested` state longer than
    /// `timeout` and revert them to `Missing` so a different peer can
    /// pick them up. Returns one entry per reclaimed chunk so the
    /// torrent loop can clear the corresponding piece's in-flight flag
    /// and free the slow peer's outstanding slot
    ///
    /// Without this, a peer that TCP-alive but stops delivering data
    /// eventually hoards every piece it touches: every chunk sits in
    /// `Requested { peer: A, .. }`, which makes `next_chunk` return
    /// `None` for the piece, which makes `drive_peer` mark the piece
    /// in-flight — and from then on `choose_requestable_piece` skips
    /// it forever. That is the primary cause of download throughput
    /// decaying over time even while peer count stays high
    pub fn reclaim_stale(&mut self, timeout: Duration) -> Vec<ReclaimedChunk> {
        let now = Instant::now();
        let chunk_size = super::super::core::CHUNK_SIZE;
        let mut out = Vec::new();
        for (&piece_idx, states) in self.pieces.iter_mut() {
            for (ci, s) in states.iter_mut().enumerate() {
                if let ChunkState::Requested { peer, since } = *s {
                    if now.duration_since(since) >= timeout {
                        *s = ChunkState::Missing;
                        out.push(ReclaimedChunk {
                            piece: piece_idx,
                            chunk_index: ci as u32,
                            begin: (ci as u32) * chunk_size,
                            peer,
                        });
                    }
                }
            }
        }
        out
    }

    /// Drop all chunk state for a piece. Called once the piece is
    /// verified and marked locally owned, because the dense state
    /// vector is only useful while the piece is being fetched.
    /// Prevents `self.pieces` from growing unbounded with completed
    /// pieces whose entries would otherwise inflate `release_peer`
    /// and `pending_chunks` into `O(total_pieces_ever_touched)`
    pub fn forget_piece(&mut self, piece: ValidPieceIndex) {
        self.pieces.remove(&piece.get());
    }

    fn states_for(&mut self, piece: ValidPieceIndex) -> &mut Vec<ChunkState> {
        let lengths = &self.lengths;
        self.pieces.entry(piece.get()).or_insert_with(|| {
            let count = lengths.chunks_of(piece).count();
            vec![ChunkState::Missing; count]
        })
    }
}

// The `states_for` borrow above conflicts with the endgame scan that also
// borrows `self.pieces`. Work around with an explicit helper that takes
// `&mut HashMap<…>`
impl ChunkTracker {
    fn _ensure<'a>(
        pieces: &'a mut HashMap<u32, Vec<ChunkState>>,
        lengths: &Lengths,
        piece: ValidPieceIndex,
    ) -> &'a mut Vec<ChunkState> {
        pieces.entry(piece.get()).or_insert_with(|| {
            let count = lengths.chunks_of(piece).count();
            vec![ChunkState::Missing; count]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_then_completes() {
        let l = Lengths::new(64 * 1024, 32 * 1024).unwrap();
        let mut t = ChunkTracker::new(l);
        let p0 = l.validate_piece(0).unwrap();

        let r0 = t.next_chunk(p0, 1).unwrap();
        assert_eq!(r0.info.chunk_index, 0);
        let r1 = t.next_chunk(p0, 1).unwrap();
        assert_eq!(r1.info.chunk_index, 1);

        assert!(!t.mark_received(r0.info));
        assert!(t.mark_received(r1.info));
    }

    #[test]
    fn endgame_duplicates_request() {
        let l = Lengths::new(64 * 1024, 32 * 1024).unwrap();
        let mut t = ChunkTracker::new(l);
        let p = l.validate_piece(0).unwrap();
        // Peer 1 requests both chunks
        let _a = t.next_chunk(p, 1).unwrap();
        let _b = t.next_chunk(p, 1).unwrap();
        // Without endgame, peer 2 has nothing to request
        assert!(t.next_chunk(p, 2).is_none());
        // With endgame, peer 2 duplicates peer 1's oldest outstanding request
        t.set_endgame(true);
        let dup = t.next_chunk(p, 2).unwrap();
        assert_eq!(dup.info.chunk_index, 0);
    }

    #[test]
    fn release_frees_requests() {
        let l = Lengths::new(64 * 1024, 32 * 1024).unwrap();
        let mut t = ChunkTracker::new(l);
        let p = l.validate_piece(0).unwrap();
        let _ = t.next_chunk(p, 1).unwrap();
        t.release_peer(1);
        let again = t.next_chunk(p, 2).unwrap();
        assert_eq!(again.info.chunk_index, 0);
    }

    #[test]
    fn release_preserves_received_from_other_peers() {
        // Bug 3 regression guard: when peer B disconnects, peer A's already
        // Received chunks must stay Received so the in-memory PieceAssembly
        // we keep around remains consistent with the chunk-state vec
        let l = Lengths::new(64 * 1024, 32 * 1024).unwrap();
        let mut t = ChunkTracker::new(l);
        let p = l.validate_piece(0).unwrap();
        let r0 = t.next_chunk(p, 1).unwrap(); // peer 1 takes chunk 0
        let _r1 = t.next_chunk(p, 2).unwrap(); // peer 2 takes chunk 1
                                               // Peer 1 delivers chunk 0
        assert!(!t.mark_received(r0.info));
        // Peer 2 disconnects; its chunk 1 reverts to Missing but chunk 0
        // (Received from peer 1) must remain Received
        let freed = t.release_peer(2);
        assert_eq!(freed, vec![0]);
        // A new peer can now grab chunk 1 and complete the piece
        let r1b = t.next_chunk(p, 3).unwrap();
        assert_eq!(r1b.info.chunk_index, 1);
        assert!(t.mark_received(r1b.info));
    }

    #[test]
    fn reclaim_stale_reverts_to_missing() {
        // Torrent with a 32 KiB piece => 2 chunks
        let l = Lengths::new(32 * 1024, 32 * 1024).unwrap();
        let mut t = ChunkTracker::new(l);
        let p = l.validate_piece(0).unwrap();

        let _r0 = t.next_chunk(p, 42).unwrap();
        let _r1 = t.next_chunk(p, 42).unwrap();
        // No staleness yet: nothing exceeds a 1 h threshold
        assert!(t.reclaim_stale(Duration::from_secs(3600)).is_empty());

        // Zero threshold reclaims everything outstanding
        std::thread::sleep(Duration::from_millis(2));
        let reclaimed = t.reclaim_stale(Duration::from_millis(1));
        assert_eq!(reclaimed.len(), 2);
        assert!(reclaimed.iter().all(|r| r.peer == 42 && r.piece == 0));

        // After reclaim, another peer can grab the first chunk again
        let fresh = t.next_chunk(p, 99).unwrap();
        assert_eq!(fresh.info.chunk_index, 0);
    }

    #[test]
    fn forget_piece_drops_state() {
        let l = Lengths::new(64 * 1024, 32 * 1024).unwrap();
        let mut t = ChunkTracker::new(l);
        let p = l.validate_piece(0).unwrap();
        let _ = t.next_chunk(p, 1);
        assert_eq!(t.pending_chunks(), 2);
        t.forget_piece(p);
        assert_eq!(t.pending_chunks(), 0);
    }
}
