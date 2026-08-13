//! Piece / chunk length arithmetic: fixed-size pieces (per torrent) split into 16 KiB chunks (BEP-3 convention)

/// Standard chunk (block) size; BEP-3 requires accepting requests up to this size and peers refuse anything larger
pub const CHUNK_SIZE: u32 = 16 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum LengthError {
    #[error("torrent has zero total length")]
    ZeroLength,
    #[error("torrent piece length is zero")]
    ZeroPieceLength,
    #[error("piece index {0} is out of range")]
    BadPieceIndex(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChunkInfo {
    pub piece_index: ValidPieceIndex,
    /// Index of chunk within the piece
    pub chunk_index: u32,
    /// Global chunk index starting at 0 for the first chunk of piece 0
    pub absolute_index: u64,
    /// Number of bytes in this chunk
    pub size: u32,
    /// Offset of the chunk's first byte within the piece
    pub offset: u32,
}

/// A piece index validated against a given [`Lengths`], acting as a proof obligation for downstream arithmetic
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ValidPieceIndex(u32);

impl std::fmt::Debug for ValidPieceIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "p{}", self.0)
    }
}
impl std::fmt::Display for ValidPieceIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl ValidPieceIndex {
    pub const fn get(self) -> u32 {
        self.0
    }
    pub const fn get_usize(self) -> usize {
        self.0 as usize
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Lengths {
    total_length: u64,
    piece_length: u32,
    last_piece_index: u32,
    last_piece_length: u32,
    chunks_per_piece: u32,
}

impl Lengths {
    pub fn new(total_length: u64, piece_length: u32) -> Result<Self, LengthError> {
        if total_length == 0 {
            return Err(LengthError::ZeroLength);
        }
        if piece_length == 0 {
            return Err(LengthError::ZeroPieceLength);
        }
        let total_pieces = total_length.div_ceil(piece_length as u64);
        let chunks_per_piece = (piece_length as u64).div_ceil(CHUNK_SIZE as u64) as u32;
        let rem = (total_length % piece_length as u64) as u32;
        let last_piece_length = if rem == 0 { piece_length } else { rem };
        Ok(Self {
            total_length,
            piece_length,
            last_piece_index: (total_pieces - 1) as u32,
            last_piece_length,
            chunks_per_piece,
        })
    }

    pub const fn total_length(&self) -> u64 {
        self.total_length
    }
    pub const fn piece_length(&self) -> u32 {
        self.piece_length
    }
    pub const fn total_pieces(&self) -> u32 {
        self.last_piece_index + 1
    }
    pub const fn chunks_per_piece(&self) -> u32 {
        self.chunks_per_piece
    }
    pub const fn piece_bitfield_bytes(&self) -> usize {
        (self.total_pieces() as usize).div_ceil(8)
    }

    pub fn validate_piece(&self, idx: u32) -> Result<ValidPieceIndex, LengthError> {
        if idx <= self.last_piece_index {
            Ok(ValidPieceIndex(idx))
        } else {
            Err(LengthError::BadPieceIndex(idx))
        }
    }

    pub fn piece_length_of(&self, idx: ValidPieceIndex) -> u32 {
        if idx.0 == self.last_piece_index {
            self.last_piece_length
        } else {
            self.piece_length
        }
    }

    /// Absolute byte offset of a piece's first byte within the torrent
    pub fn piece_offset(&self, idx: ValidPieceIndex) -> u64 {
        idx.0 as u64 * self.piece_length as u64
    }

    /// Iterate chunks that make up a piece
    pub fn chunks_of(&self, idx: ValidPieceIndex) -> impl Iterator<Item = ChunkInfo> + '_ {
        let piece_len = self.piece_length_of(idx);
        let chunk_count = piece_len.div_ceil(CHUNK_SIZE);
        let piece_index = idx;
        let chunks_per_normal = self.chunks_per_piece as u64;
        (0..chunk_count).map(move |c| {
            let offset = c * CHUNK_SIZE;
            let size = piece_len.saturating_sub(offset).min(CHUNK_SIZE);
            ChunkInfo {
                piece_index,
                chunk_index: c,
                absolute_index: piece_index.0 as u64 * chunks_per_normal + c as u64,
                size,
                offset,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_math() {
        let l = Lengths::new(1_000_000, 262_144).unwrap();
        assert_eq!(l.total_pieces(), 4);
        let last = l.validate_piece(3).unwrap();
        assert_eq!(l.piece_length_of(last), 213_568);
        let first = l.validate_piece(0).unwrap();
        assert_eq!(l.piece_length_of(first), 262_144);
    }

    #[test]
    fn exact_multiple() {
        let l = Lengths::new(CHUNK_SIZE as u64 * 4, CHUNK_SIZE).unwrap();
        assert_eq!(l.total_pieces(), 4);
        let last = l.validate_piece(3).unwrap();
        assert_eq!(l.piece_length_of(last), CHUNK_SIZE);
    }

    #[test]
    fn chunks_of_last_piece_is_truncated() {
        let l = Lengths::new(CHUNK_SIZE as u64 * 2 + 100, CHUNK_SIZE * 2).unwrap();
        let last = l.validate_piece(1).unwrap();
        let chunks: Vec<_> = l.chunks_of(last).collect();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].size, 100);
        assert_eq!(chunks[0].offset, 0);
    }

    #[test]
    fn bitfield_bytes() {
        assert_eq!(Lengths::new(7, 1).unwrap().piece_bitfield_bytes(), 1);
        assert_eq!(Lengths::new(9, 1).unwrap().piece_bitfield_bytes(), 2);
    }

    #[test]
    fn reject_zero() {
        assert!(Lengths::new(0, 1024).is_err());
        assert!(Lengths::new(1024, 0).is_err());
    }

    #[test]
    fn reject_bad_piece_index() {
        let l = Lengths::new(1024, 512).unwrap();
        assert!(l.validate_piece(5).is_err());
    }
}
