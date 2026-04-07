use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use reqwest::Client;
use tokio::io::AsyncWriteExt;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use super::decrypt::{decrypt_segment, fetch_decryption_key, iv_from_sequence};
use super::parser::Segment;
use crate::engine::speed_limiter::SpeedLimiter;

const SEGMENT_MAX_RETRIES: u32 = 5;
const PROGRESS_FILENAME: &str = ".m3u8.progress";

/// State for tracking segment download progress (for resume)
pub struct ProgressState {
    pub completed_indices: HashSet<usize>,
    progress_path: PathBuf,
}

impl ProgressState {
    /// Load or create a progress state from disk
    pub fn load(temp_dir: &Path) -> Self {
        let progress_path = temp_dir.join(PROGRESS_FILENAME);
        let completed_indices = if progress_path.exists() {
            std::fs::read_to_string(&progress_path)
                .unwrap_or_default()
                .lines()
                .filter_map(|line| line.trim().parse::<usize>().ok())
                .collect()
        } else {
            HashSet::new()
        };
        Self {
            completed_indices,
            progress_path,
        }
    }

    /// Mark a segment index as completed and persist
    pub fn mark_completed(&mut self, index: usize) {
        self.completed_indices.insert(index);
        self.persist();
    }

    fn persist(&self) {
        let content: String = self
            .completed_indices
            .iter()
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let _ = std::fs::write(&self.progress_path, content);
    }

    /// Remove the progress file
    pub fn cleanup(&self) {
        let _ = std::fs::remove_file(&self.progress_path);
    }
}

/// Key cache to avoid re-fetching the same decryption key
struct KeyCache {
    entries: std::collections::HashMap<String, [u8; 16]>,
}

impl KeyCache {
    fn new() -> Self {
        Self {
            entries: std::collections::HashMap::new(),
        }
    }

    async fn get_or_fetch(
        &mut self,
        key_uri: &str,
        client: &Client,
    ) -> Result<[u8; 16], String> {
        if let Some(key) = self.entries.get(key_uri) {
            return Ok(*key);
        }
        let key = fetch_decryption_key(key_uri, client).await?;
        self.entries.insert(key_uri.to_string(), key);
        Ok(key)
    }
}

/// Download all segments concurrently, with resume and decryption support
/// Returns paths to the downloaded segment files in order
#[allow(clippy::too_many_arguments)]
pub async fn download_segments(
    segments: &[Segment],
    media_sequence: u64,
    temp_dir: &Path,
    client: &Client,
    total: Arc<AtomicU64>,
    completed: Arc<AtomicU64>,
    connections: Arc<AtomicU32>,
    cancelled: Arc<AtomicBool>,
    cancel_token: CancellationToken,
    global_limiter: Arc<SpeedLimiter>,
    task_limiter: Arc<SpeedLimiter>,
    max_concurrent: usize,
) -> Result<(Vec<PathBuf>, ProgressState), String> {
    std::fs::create_dir_all(temp_dir)
        .map_err(|e| format!("Failed to create temp dir: {e}"))?;

    let mut progress = ProgressState::load(temp_dir);
    let semaphore = Arc::new(Semaphore::new(max_concurrent));
    let key_cache = Arc::new(tokio::sync::Mutex::new(KeyCache::new()));

    let segment_paths: Vec<PathBuf> = (0..segments.len())
        .map(|i| temp_dir.join(format!("seg_{i:06}.ts")))
        .collect();

    // Account for already-downloaded segment bytes in completed count
    let mut resumed_bytes: u64 = 0;
    for &idx in &progress.completed_indices {
        if let Some(path) = segment_paths.get(idx) {
            if let Ok(meta) = std::fs::metadata(path) {
                resumed_bytes += meta.len();
            }
        }
    }
    completed.store(resumed_bytes, Ordering::Relaxed);

    // We don't know total bytes upfront; estimate after first segment completes
    // Use AtomicBool to ensure we only estimate once
    let total_estimated = Arc::new(AtomicBool::new(false));

    let total_segments = segments.len() as u64;
    let completed_for_spawn = completed.clone();
    let total_for_spawn = total.clone();
    let total_estimated_for_spawn = total_estimated.clone();
    let total_segments_for_spawn = total_segments;

    let mut handles = Vec::with_capacity(segments.len());

    for (index, segment) in segments.iter().enumerate() {
        // Skip already-completed segments
        if progress.completed_indices.contains(&index) {
            if segment_paths[index].exists() {
                handles.push(None);
                continue;
            }
            // File missing despite progress marker —> re-download
        }

        let permit = semaphore.clone();
        let client = client.clone();
        let seg_path = segment_paths[index].clone();
        let segment = segment.clone();
        let seq = media_sequence + index as u64;
        let cancelled = cancelled.clone();
        let cancel_token = cancel_token.clone();
        let connections = connections.clone();
        let global_limiter = global_limiter.clone();
        let task_limiter = task_limiter.clone();
        let key_cache = key_cache.clone();
        let completed_inner = completed_for_spawn.clone();
        let total_inner = total_for_spawn.clone();
        let total_estimated_inner = total_estimated_for_spawn.clone();
        let total_segments_inner = total_segments_for_spawn;

        let handle = tokio::spawn(async move {
            let _permit = permit
                .acquire()
                .await
                .map_err(|_| "Semaphore closed".to_string())?;

            connections.fetch_add(1, Ordering::Relaxed);
            let result = download_single_segment(
                &client,
                &segment,
                seq,
                &seg_path,
                &cancelled,
                &cancel_token,
                &global_limiter,
                &task_limiter,
                &key_cache,
            )
            .await;
            connections.fetch_sub(1, Ordering::Relaxed);

            match result {
                Ok(bytes_written) => {
                    // Update byte-based progress
                    completed_inner.fetch_add(bytes_written, Ordering::Relaxed);

                    // Estimate total from first completed segment
                    if !total_estimated_inner.swap(true, Ordering::Relaxed) {
                        let estimated =
                            bytes_written.saturating_mul(total_segments_inner);
                        total_inner.store(estimated, Ordering::Relaxed);
                    }

                    Ok(index)
                }
                Err(e) => Err(e),
            }
        });

        handles.push(Some(handle));
    }

    // Collect results
    // On error, cancel remaining tasks via cancel_token

    for maybe_handle in handles {
        if cancelled.load(Ordering::Relaxed) || cancel_token.is_cancelled() {
            return Err("cancelled".to_string());
        }

        let Some(handle) = maybe_handle else { continue };

        match handle.await {
            Ok(Ok(index)) => {
                progress.mark_completed(index);
            }
            Ok(Err(e)) => {
                if e.contains("cancelled") {
                    return Err("cancelled".to_string());
                }
                // Cancel remaining spawned tasks before returning error
                cancel_token.cancel();
                return Err(e);
            }
            Err(e) => {
                cancel_token.cancel();
                return Err(format!("Segment task panicked: {e}"));
            }
        }
    }

    Ok((segment_paths, progress))
}

/// Download a single segment
/// Returns the number of bytes written on success
#[allow(clippy::too_many_arguments)]
async fn download_single_segment(
    client: &Client,
    segment: &Segment,
    sequence_number: u64,
    output_path: &Path,
    cancelled: &AtomicBool,
    cancel_token: &CancellationToken,
    global_limiter: &SpeedLimiter,
    task_limiter: &SpeedLimiter,
    key_cache: &Arc<tokio::sync::Mutex<KeyCache>>,
) -> Result<u64, String> {
    let mut retries = 0;

    loop {
        if cancelled.load(Ordering::Relaxed) || cancel_token.is_cancelled() {
            return Err("cancelled".to_string());
        }

        match attempt_segment_download(
            client,
            segment,
            sequence_number,
            output_path,
            global_limiter,
            task_limiter,
            key_cache,
        )
        .await
        {
            Ok(bytes) => return Ok(bytes),
            Err(e) => {
                retries += 1;
                if retries >= SEGMENT_MAX_RETRIES {
                    return Err(format!(
                        "Segment {} failed after {SEGMENT_MAX_RETRIES} retries: {e}",
                        segment.url
                    ));
                }
                let delay = std::time::Duration::from_millis(500 * 2u64.pow(retries - 1));
                tokio::time::sleep(delay).await;
            }
        }
    }
}

/// Single attempt to download a segment
/// Returns the number of bytes written on success
#[allow(clippy::too_many_arguments)]
async fn attempt_segment_download(
    client: &Client,
    segment: &Segment,
    sequence_number: u64,
    output_path: &Path,
    global_limiter: &SpeedLimiter,
    task_limiter: &SpeedLimiter,
    key_cache: &Arc<tokio::sync::Mutex<KeyCache>>,
) -> Result<u64, String> {
    let mut request = client.get(&segment.url);

    // Add byte range header if specified
    if let Some(ref br) = segment.byte_range {
        let end = br.offset + br.length - 1;
        request = request.header("Range", format!("bytes={}-{}", br.offset, end));
    }

    let resp = request
        .send()
        .await
        .map_err(|e| format!("Segment request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("Segment HTTP {}", resp.status()));
    }

    let data = resp
        .bytes()
        .await
        .map_err(|e| format!("Failed to read segment body: {e}"))?;

    // Apply speed limiting
    let chunk_size = data.len();
    global_limiter.acquire(chunk_size).await;
    task_limiter.acquire(chunk_size).await;

    // Decrypt if needed
    let final_data = if let Some(ref enc) = segment.encryption {
        if enc.method == "AES-128" {
            let key = key_cache.lock().await.get_or_fetch(&enc.key_uri, client).await?;
            let iv = match &enc.iv {
                Some(iv_bytes) => {
                    let mut iv = [0u8; 16];
                    let len = iv_bytes.len().min(16);
                    iv[16 - len..].copy_from_slice(&iv_bytes[..len]);
                    iv
                }
                None => iv_from_sequence(sequence_number),
            };
            decrypt_segment(&data, &key, &iv)?
        } else {
            // or unknown — pass through
            data.to_vec()
        }
    } else {
        data.to_vec()
    };

    // Write to disk
    let bytes_written = final_data.len() as u64;
    let mut file = tokio::fs::File::create(output_path)
        .await
        .map_err(|e| format!("Failed to create segment file: {e}"))?;
    file.write_all(&final_data)
        .await
        .map_err(|e| format!("Failed to write segment: {e}"))?;
    file.flush()
        .await
        .map_err(|e| format!("Failed to flush segment: {e}"))?;

    Ok(bytes_written)
}
