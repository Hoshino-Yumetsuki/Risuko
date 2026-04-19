//! Per-chunk state machine: schedules outstanding chunk requests and
//! coordinates endgame (duplicate requests to multiple peers)

use std::collections::HashMap;
use std::time::Instant;

use super::super::core::lengths::{ChunkInfo, Lengths, ValidPieceIndex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkState {
    Missing,
    Requested { peer: u32, since: Instant },
    Received,
}

#[derive(Debug, Clone, Copy)]
pub struct ChunkRequest {
    pub info: ChunkInfo,
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
    /// endgame mode, `Requested` chunks are also returned (duplicated)
    pub fn next_chunk(&mut self, piece: ValidPieceIndex, peer: u32) -> Option<ChunkRequest> {
        let endgame = self.endgame;
        let states = self.states_for(piece);
        let mut candidate: Option<usize> = None;
        for (i, s) in states.iter().enumerate() {
            match s {
                ChunkState::Missing => {
                    candidate = Some(i);
                    break;
                }
                ChunkState::Requested { peer: p, .. } if endgame && *p != peer => {
                    if candidate.is_none() {
                        candidate = Some(i);
                    }
                }
                _ => {}
            }
        }
        let i = candidate?;
        let info = self
            .lengths
            .chunks_of(piece)
            .nth(i)
            .expect("chunk index within piece");
        let states = self.pieces.get_mut(&piece.get()).unwrap();
        states[i] = ChunkState::Requested {
            peer,
            since: Instant::now(),
        };
        Some(ChunkRequest { info })
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
    /// Without this, a try_send failure would permanently strand the chunk
    /// in `Requested` state until the peer disconnects
    pub fn unrequest_chunk(&mut self, piece: ValidPieceIndex, chunk_index: u32) {
        if let Some(states) = self.pieces.get_mut(&piece.get()) {
            if let Some(s) = states.get_mut(chunk_index as usize) {
                if matches!(s, ChunkState::Requested { .. }) {
                    *s = ChunkState::Missing;
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
}
