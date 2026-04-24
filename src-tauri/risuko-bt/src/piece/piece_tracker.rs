//! Per-piece availability tracking (rarest-first selection)
//!
//! Keeps, for each piece, a count of how many connected peers advertise it
//! and whether we already own it locally. `choose_piece` returns the rarest
//! piece not yet owned that at least one peer has
//!
//! Pieces have three logical states:
//! - **LOCAL**: verified and stored on disk (`have_local`)
//! - **IN-FLIGHT**: all chunks requested but not yet verified (`in_flight`)
//! - **REQUESTABLE**: neither local nor fully in-flight
//!
//! `choose_requestable_piece` only returns REQUESTABLE pieces, so different
//! peers get different work. `choose_piece` (used for interest checks) ignores
//! the in-flight flag

use super::super::core::lengths::{Lengths, ValidPieceIndex};

#[derive(Debug)]
pub struct PieceTracker {
    lengths: Lengths,
    have_local: Vec<bool>,
    /// Pieces where all chunks have been requested but the piece hasn't been
    /// hash-verified yet. Cleared on verify success, verify failure, or when
    /// a peer owning chunks disconnects
    in_flight: Vec<bool>,
    /// How many peers advertise each piece
    availability: Vec<u32>,
    /// Scratch buffer reused by `choose_piece` to avoid allocations
    scratch: Vec<ValidPieceIndex>,
}

impl PieceTracker {
    pub fn new(lengths: Lengths) -> Self {
        let n = lengths.total_pieces() as usize;
        Self {
            lengths,
            have_local: vec![false; n],
            in_flight: vec![false; n],
            availability: vec![0; n],
            scratch: Vec::with_capacity(n.min(1024)),
        }
    }

    pub fn lengths(&self) -> &Lengths {
        &self.lengths
    }

    pub fn set_local(&mut self, idx: ValidPieceIndex, have: bool) {
        self.have_local[idx.get_usize()] = have;
        if have {
            // No longer in-flight once verified
            self.in_flight[idx.get_usize()] = false;
        }
    }

    pub fn has_local(&self, idx: ValidPieceIndex) -> bool {
        self.have_local[idx.get_usize()]
    }

    /// Mark a piece as fully in-flight (all chunks requested, awaiting hash
    /// verification). `choose_requestable_piece` will skip it
    pub fn mark_in_flight(&mut self, idx: ValidPieceIndex) {
        self.in_flight[idx.get_usize()] = true;
    }

    /// Clear the in-flight flag so the piece can be re-requested (e.g. after
    /// hash failure or peer disconnect freeing chunks)
    pub fn clear_in_flight(&mut self, idx: ValidPieceIndex) {
        self.in_flight[idx.get_usize()] = false;
    }

    pub fn bitfield(&self) -> Vec<u8> {
        let mut out = vec![0u8; self.lengths.piece_bitfield_bytes()];
        for (i, have) in self.have_local.iter().enumerate() {
            if *have {
                let byte = i / 8;
                let bit = 7 - (i % 8);
                out[byte] |= 1 << bit;
            }
        }
        out
    }

    /// Increment the availability counter for every bit set in `bitfield`
    pub fn add_peer_bitfield(&mut self, bitfield: &[u8]) {
        let n = self.lengths.total_pieces() as usize;
        for i in 0..n {
            let byte = i / 8;
            let bit = 7 - (i % 8);
            if bitfield.get(byte).is_some_and(|b| b & (1 << bit) != 0) {
                self.availability[i] = self.availability[i].saturating_add(1);
            }
        }
    }

    /// Inverse of `add_peer_bitfield`, called when a peer disconnects
    pub fn remove_peer_bitfield(&mut self, bitfield: &[u8]) {
        let n = self.lengths.total_pieces() as usize;
        for i in 0..n {
            let byte = i / 8;
            let bit = 7 - (i % 8);
            if bitfield.get(byte).is_some_and(|b| b & (1 << bit) != 0) {
                self.availability[i] = self.availability[i].saturating_sub(1);
            }
        }
    }

    pub fn note_peer_has(&mut self, idx: ValidPieceIndex) {
        let slot = &mut self.availability[idx.get_usize()];
        *slot = slot.saturating_add(1);
    }

    pub fn note_peer_lost(&mut self, idx: ValidPieceIndex) {
        let slot = &mut self.availability[idx.get_usize()];
        *slot = slot.saturating_sub(1);
    }

    pub fn is_complete(&self) -> bool {
        self.have_local.iter().all(|b| *b)
    }

    pub fn completed_pieces(&self) -> u32 {
        self.have_local.iter().filter(|b| **b).count() as u32
    }

    /// Pick the rarest piece the peer has that we don't, breaking ties with
    /// the lowest index. Returns None if the peer has nothing useful
    ///
    /// Used for interest checks — does NOT skip in-flight pieces
    ///
    /// `peer_bitfield` is indexed like `bitfield()`: MSB-first within bytes
    pub fn choose_piece(&mut self, peer_bitfield: &[u8]) -> Option<ValidPieceIndex> {
        self.choose_impl(peer_bitfield, false, 0, None)
    }

    /// Like [`choose_piece`] but skips pieces whose index is in `exclude`,
    /// and uses `hint` to break ties among equally-rare pieces (so different
    /// peers spread across the candidate bucket instead of all picking the
    /// same stalled piece first in endgame)
    pub fn choose_piece_excluding(
        &mut self,
        peer_bitfield: &[u8],
        hint: u32,
        exclude: &std::collections::HashSet<u32>,
    ) -> Option<ValidPieceIndex> {
        self.choose_impl(peer_bitfield, false, hint, Some(exclude))
    }

    /// Pick the rarest requestable piece the peer has. Skips both locally
    /// owned and fully in-flight pieces. `hint` is used to break ties among
    /// equally-rare pieces so that different peers get different work
    pub fn choose_requestable_piece(
        &mut self,
        peer_bitfield: &[u8],
        hint: u32,
    ) -> Option<ValidPieceIndex> {
        self.choose_impl(peer_bitfield, true, hint, None)
    }

    fn choose_impl(
        &mut self,
        peer_bitfield: &[u8],
        skip_in_flight: bool,
        hint: u32,
        exclude: Option<&std::collections::HashSet<u32>>,
    ) -> Option<ValidPieceIndex> {
        self.scratch.clear();
        let mut rarest: Option<u32> = None;
        let n = self.lengths.total_pieces() as usize;
        for i in 0..n {
            let byte = i / 8;
            let bit = 7 - (i % 8);
            if peer_bitfield.get(byte).is_none_or(|b| b & (1 << bit) == 0) {
                continue;
            }
            if self.have_local[i] {
                continue;
            }
            if skip_in_flight && self.in_flight[i] {
                continue;
            }
            if exclude.is_some_and(|e| e.contains(&(i as u32))) {
                continue;
            }
            let avail = self.availability[i];
            if avail == 0 {
                continue;
            }
            match rarest {
                None => {
                    rarest = Some(avail);
                    self.scratch.clear();
                    self.scratch
                        .push(self.lengths.validate_piece(i as u32).unwrap());
                }
                Some(r) if avail < r => {
                    rarest = Some(avail);
                    self.scratch.clear();
                    self.scratch
                        .push(self.lengths.validate_piece(i as u32).unwrap());
                }
                Some(r) if avail == r => {
                    self.scratch
                        .push(self.lengths.validate_piece(i as u32).unwrap());
                }
                _ => {}
            }
        }
        if self.scratch.is_empty() {
            return None;
        }
        // Distribute piece selection across peers to maximize parallelism
        let idx = hint as usize % self.scratch.len();
        Some(self.scratch[idx])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lengths(pieces: u32) -> Lengths {
        Lengths::new((pieces as u64) * 1024, 1024).unwrap()
    }

    #[test]
    fn bitfield_round_trip() {
        let mut t = PieceTracker::new(lengths(9));
        t.set_local(t.lengths.validate_piece(0).unwrap(), true);
        t.set_local(t.lengths.validate_piece(8).unwrap(), true);
        let bf = t.bitfield();
        assert_eq!(bf.len(), 2);
        assert_eq!(bf[0], 0b1000_0000);
        assert_eq!(bf[1], 0b1000_0000);
    }

    #[test]
    fn rarest_first_choice() {
        let mut t = PieceTracker::new(lengths(4));
        // Peer advertises all four pieces
        t.add_peer_bitfield(&[0b1111_0000]);
        // Another peer has piece 0 and 1 only
        t.add_peer_bitfield(&[0b1100_0000]);
        // So availability: [2, 2, 1, 1]. We should pick piece 2 (lowest rarest)
        let chosen = t.choose_piece(&[0b1111_0000]).unwrap();
        assert_eq!(chosen.get(), 2);

        // After we own piece 2, piece 3 should win
        t.set_local(chosen, true);
        let chosen = t.choose_piece(&[0b1111_0000]).unwrap();
        assert_eq!(chosen.get(), 3);
    }

    #[test]
    fn peer_with_nothing_useful() {
        let mut t = PieceTracker::new(lengths(4));
        t.add_peer_bitfield(&[0b1111_0000]);
        // Peer advertises piece 0 only, which we've already got.
        t.set_local(t.lengths.validate_piece(0).unwrap(), true);
        assert!(t.choose_piece(&[0b1000_0000]).is_none());
    }

    #[test]
    fn completion_tracking() {
        let mut t = PieceTracker::new(lengths(3));
        assert!(!t.is_complete());
        for i in 0..3 {
            t.set_local(t.lengths.validate_piece(i).unwrap(), true);
        }
        assert!(t.is_complete());
        assert_eq!(t.completed_pieces(), 3);
    }
}
