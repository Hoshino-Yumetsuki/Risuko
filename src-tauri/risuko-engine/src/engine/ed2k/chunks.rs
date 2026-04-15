use std::path::PathBuf;
use tokio::fs::OpenOptions;
use tokio::io::{AsyncSeekExt, AsyncWriteExt, SeekFrom};

use super::types::*;

/// Manages chunk-level download state and disk I/O for an ed2k file
pub struct ChunkManager {
    file_path: PathBuf,
    file_size: u64,
    chunk_count: u64,
    chunk_hashes: Vec<[u8; 16]>,
    chunk_status: Vec<ChunkStatus>,
    completed_length: u64,
}

impl ChunkManager {
    pub fn new(file_path: PathBuf, file_size: u64) -> Self {
        let count = chunk_count(file_size);
        Self {
            file_path,
            file_size,
            chunk_count: count,
            chunk_hashes: Vec::new(),
            chunk_status: vec![ChunkStatus::Missing; count as usize],
            completed_length: 0,
        }
    }

    pub fn completed_length(&self) -> u64 {
        self.completed_length
    }

    pub fn is_complete(&self) -> bool {
        self.completed_length >= self.file_size
    }

    /// Set chunk hashes received from a peer's Hashset Answer
    pub fn set_chunk_hashes(&mut self, hashes: Vec<[u8; 16]>) {
        self.chunk_hashes = hashes;
    }

    /// Get the byte range for a given chunk index
    pub fn chunk_range(&self, index: u64) -> (u64, u64) {
        let start = index * ED2K_CHUNK_SIZE;
        let end = std::cmp::min(start + ED2K_CHUNK_SIZE, self.file_size);
        (start, end)
    }

    /// Find the next chunk to download (Missing state, peer has it)
    pub fn next_needed_chunk(&self, peer_parts: &[bool]) -> Option<u64> {
        for i in 0..self.chunk_count as usize {
            if self.chunk_status[i] == ChunkStatus::Missing {
                if peer_parts.is_empty() || (i < peer_parts.len() && peer_parts[i]) {
                    return Some(i as u64);
                }
            }
        }
        None
    }

    /// Mark a chunk as downloaded
    pub fn mark_downloaded(&mut self, index: u64) {
        if (index as usize) < self.chunk_status.len() {
            self.chunk_status[index as usize] = ChunkStatus::Downloaded;
        }
    }

    /// Create or open the output file, pre-allocating the full size
    pub async fn init_file(&self) -> Result<(), String> {
        if let Some(parent) = self.file_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| format!("Failed to create dir: {}", e))?;
        }

        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .open(&self.file_path)
            .await
            .map_err(|e| format!("Failed to create file: {}", e))?;

        // Pre-allocate
        file.set_len(self.file_size)
            .await
            .map_err(|e| format!("Failed to pre-allocate: {}", e))?;

        Ok(())
    }

    /// Write data at an absolute file offset
    pub async fn write_data(&mut self, offset: u64, data: &[u8]) -> Result<(), String> {
        let end = offset.saturating_add(data.len() as u64);
        if end > self.file_size {
            return Err(format!(
                "Write out of bounds: offset {} + len {} exceeds file size {}",
                offset,
                data.len(),
                self.file_size
            ));
        }

        let mut file = OpenOptions::new()
            .write(true)
            .open(&self.file_path)
            .await
            .map_err(|e| format!("Failed to open file for writing: {}", e))?;

        file.seek(SeekFrom::Start(offset))
            .await
            .map_err(|e| format!("Seek failed: {}", e))?;

        file.write_all(data)
            .await
            .map_err(|e| format!("Write failed: {}", e))?;

        self.completed_length = (self.completed_length + data.len() as u64).min(self.file_size);

        // Update chunk status
        let chunk_idx = offset / ED2K_CHUNK_SIZE;
        let chunk_end = std::cmp::min((chunk_idx + 1) * ED2K_CHUNK_SIZE, self.file_size);
        if self.completed_length >= chunk_end {
            self.mark_downloaded(chunk_idx);
        }

        Ok(())
    }
}
