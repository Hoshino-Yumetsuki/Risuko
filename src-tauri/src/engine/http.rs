use std::fs;
use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use futures_util::StreamExt;
use reqwest::header::{
    HeaderMap, HeaderName, HeaderValue, ACCEPT_RANGES, CONTENT_LENGTH, CONTENT_RANGE, ETAG, IF_MATCH, LAST_MODIFIED, RANGE,
};
use reqwest::Client;
use serde_json::{Map, Value};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use super::speed_limiter::SpeedLimiter;

const PART_SUFFIX: &str = ".part";
/// Minimum segment size: 1 MiB. Don't split below this
const MIN_SEGMENT_SIZE: u64 = 1024 * 1024;
/// Buffer capacity per chunk for in-memory buffering before flush
const CHUNK_BUF_CAPACITY: usize = 2 * 1024 * 1024;
/// Max retries per chunk on transient errors
const CHUNK_MAX_RETRIES: u32 = 5;
/// Error substring returned when a stale .part was removed
const STALE_PART_REMOVED: &str = "stale partial file removed";
/// Exponential moving average smoothing factor for speed reporting
const SPEED_EMA_ALPHA: f64 = 0.3;

/// Inclusive byte range [start, end]
#[derive(Debug, Clone, Copy)]
struct ChunkRange {
    start: u64,
    end: u64,
}

impl ChunkRange {
    fn new(start: u64, end: u64) -> Self {
        debug_assert!(end >= start);
        Self { start, end }
    }

    fn len(&self) -> u64 {
        self.end - self.start + 1
    }

    fn to_range_header_value(&self) -> String {
        format!("bytes={}-{}", self.start, self.end)
    }
}

/// Build a reqwest Client with common settings applied from options
fn build_client(options: &Map<String, Value>) -> Result<Client, String> {
    let ua = options
        .get("user-agent")
        .and_then(|v| v.as_str())
        .unwrap_or("Mozilla/5.0");

    let mut builder = Client::builder()
        .user_agent(ua)
        .redirect(reqwest::redirect::Policy::limited(10))
        .connect_timeout(std::time::Duration::from_secs(30))
        .tcp_nodelay(true)
        .gzip(true)
        .brotli(true)
        .deflate(true);

    if let Some(proxy_url) = options
        .get("all-proxy")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        if let Ok(proxy) = reqwest::Proxy::all(proxy_url) {
            builder = builder.proxy(proxy);
        }
    }

    builder
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {e}"))
}

/// Build custom headers from options
fn build_headers(options: &Map<String, Value>) -> HeaderMap {
    let mut headers = HeaderMap::new();

    if let Some(header_val) = options.get("header").and_then(|v| v.as_str()) {
        for h in header_val.split('\n') {
            let trimmed = h.trim();
            if let Some(colon) = trimmed.find(':') {
                let name = trimmed[..colon].trim();
                let value = trimmed[colon + 1..].trim();
                if let (Ok(n), Ok(v)) = (
                    HeaderName::from_bytes(name.as_bytes()),
                    HeaderValue::from_str(value),
                ) {
                    headers.insert(n, v);
                }
            }
        }
    }

    if let Some(referer) = options
        .get("referer")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        if let Ok(v) = HeaderValue::from_str(referer) {
            headers.insert(reqwest::header::REFERER, v);
        }
    }

    if let Some(cookie) = options
        .get("cookie")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        if let Ok(v) = HeaderValue::from_str(cookie) {
            headers.insert(reqwest::header::COOKIE, v);
        }
    }

    headers
}

/// Run an HTTP/FTP download. This is the main entry point called from manager.rs
/// Returns the final file path on success
pub async fn run_http_download(
    uri: &str,
    dir: &str,
    out: &str,
    options: &Map<String, Value>,
    total: Arc<AtomicU64>,
    completed: Arc<AtomicU64>,
    speed: Arc<AtomicU64>,
    cancelled: Arc<AtomicBool>,
    connections: Arc<AtomicU32>,
    cancel_token: CancellationToken,
    global_limiter: Arc<SpeedLimiter>,
    task_limiter: Arc<SpeedLimiter>,
) -> Result<PathBuf, String> {
    tracing::info!("Starting download: uri={uri}, dir={dir}, out={out}");
    let dir_path = Path::new(dir);
    fs::create_dir_all(dir_path).map_err(|e| format!("Failed to create dir: {e}"))?;

    let filename = if out.is_empty() {
        infer_filename_from_uri(uri)
    } else {
        out.to_string()
    };

    let part_name = if filename.ends_with(PART_SUFFIX) {
        filename.clone()
    } else {
        format!("{filename}{PART_SUFFIX}")
    };
    let part_path = dir_path.join(&part_name);

    let split = options
        .get("split")
        .and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
        .unwrap_or(1)
        .max(1) as usize;

    let client = build_client(options)?;
    let headers = build_headers(options);

    let use_remote_time = options
        .get("remote-time")
        .and_then(|v| v.as_bool().or_else(|| v.as_str().map(|s| s == "true")))
        .unwrap_or(false);

    // Check for existing partial download
    let existing_size = if part_path.exists() {
        fs::metadata(&part_path).map(|m| m.len()).unwrap_or(0)
    } else {
        0
    };

    let mut last_modified_header: Option<String> = None;

    let is_http = uri.starts_with("http://") || uri.starts_with("https://");
    if is_http && split > 1 && existing_size == 0 {
        match probe_range_support(&client, uri, &headers).await {
            Ok(Some(probe)) if probe.content_length > MIN_SEGMENT_SIZE * split as u64 => {
                if use_remote_time {
                    last_modified_header = probe.last_modified.clone();
                }
                connections.store(split as u32, Ordering::Relaxed);
                let result = run_multi_chunk(
                    &client,
                    uri,
                    &part_path,
                    probe.content_length,
                    split,
                    &headers,
                    total,
                    completed,
                    speed,
                    cancel_token,
                    &filename,
                    dir_path,
                    global_limiter,
                    task_limiter,
                    probe.etag,
                )
                .await;
                if let Ok(ref path) = result {
                    if let Some(ref lm) = last_modified_header {
                        apply_remote_file_time(path, lm);
                    }
                }
                return result;
            }
            Ok(Some(_)) => {
                tracing::info!("File too small for multi-chunk, using single connection");
            }
            Ok(None) => {
                tracing::info!("Server does not support ranges, using single connection");
            }
            Err(e) => {
                tracing::warn!("Range probe failed, falling back to single: {e}");
            }
        }
    }

    // Single-connection download (fallback, resume, FTP, or small files)
    connections.store(1, Ordering::Relaxed);
    let result = run_single_download(
        &client,
        uri,
        &part_path,
        &headers,
        total.clone(),
        completed.clone(),
        speed.clone(),
        cancelled.clone(),
        cancel_token.clone(),
        &filename,
        dir_path,
        global_limiter.clone(),
        task_limiter.clone(),
    )
    .await;

    // If a stale .part was removed, retry once from scratch
    let final_result = match result {
        Err(ref e) if e.contains(STALE_PART_REMOVED) => {
            tracing::info!("Retrying download after stale .part removal");
            completed.store(0, Ordering::Relaxed);
            total.store(0, Ordering::Relaxed);
            speed.store(0, Ordering::Relaxed);

            if is_http && split > 1 {
                if let Ok(Some(probe)) =
                    probe_range_support(&client, uri, &headers).await
                {
                    if probe.content_length > MIN_SEGMENT_SIZE * split as u64 {
                        if use_remote_time {
                            last_modified_header = probe.last_modified.clone();
                        }
                        connections.store(split as u32, Ordering::Relaxed);
                        let mc_result = run_multi_chunk(
                            &client,
                            uri,
                            &part_path,
                            probe.content_length,
                            split,
                            &headers,
                            total,
                            completed,
                            speed,
                            cancel_token,
                            &filename,
                            dir_path,
                            global_limiter,
                            task_limiter,
                            probe.etag,
                        )
                        .await;
                        if let Ok(ref path) = mc_result {
                            if let Some(ref lm) = last_modified_header {
                                apply_remote_file_time(path, lm);
                            }
                        }
                        return mc_result;
                    }
                }
            }

            connections.store(1, Ordering::Relaxed);
            run_single_download(
                &client,
                uri,
                &part_path,
                &headers,
                total,
                completed,
                speed,
                cancelled,
                cancel_token,
                &filename,
                dir_path,
                global_limiter,
                task_limiter,
            )
            .await
        }
        other => other,
    };

    match final_result {
        Ok((path, lm)) => {
            if use_remote_time {
                if let Some(ref lm_str) = lm {
                    apply_remote_file_time(&path, lm_str);
                }
            }
            Ok(path)
        }
        Err(e) => Err(e),
    }
}

/// Result from probing range support: content length + optional ETag
struct ProbeResult {
    content_length: u64,
    etag: Option<String>,
    last_modified: Option<String>,
}

/// Probe whether the server supports Range requests
/// Returns Some(ProbeResult) if ranges are supported, None otherwise
async fn probe_range_support(
    client: &Client,
    uri: &str,
    headers: &HeaderMap,
) -> Result<Option<ProbeResult>, String> {
    let resp = client
        .get(uri)
        .headers(headers.clone())
        .header(RANGE, "bytes=0-0")
        .send()
        .await
        .map_err(|e| format!("Range probe request failed: {e}"))?;

    let status = resp.status().as_u16();
    let etag = resp
        .headers()
        .get(ETAG)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let last_modified = resp
        .headers()
        .get(LAST_MODIFIED)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    if status == 206 {
        // Parse Content-Range: bytes 0-0/TOTAL
        if let Some(cr) = resp
            .headers()
            .get(CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
        {
            if let Some(slash) = cr.rfind('/') {
                if let Ok(total) = cr[slash + 1..].trim().parse::<u64>() {
                    return Ok(Some(ProbeResult { content_length: total, etag, last_modified }));
                }
            }
        }
        return Ok(None);
    }

    if status == 200 {
        // Server ignored Range header, check Accept-Ranges
        if let Some(ar) = resp
            .headers()
            .get(ACCEPT_RANGES)
            .and_then(|v| v.to_str().ok())
        {
            if ar.eq_ignore_ascii_case("bytes") {
                if let Some(cl) = resp
                    .headers()
                    .get(CONTENT_LENGTH)
                    .and_then(|v| v.to_str().ok())
                {
                    if let Ok(total) = cl.trim().parse::<u64>() {
                        if total > 0 {
                            return Ok(Some(ProbeResult { content_length: total, etag, last_modified }));
                        }
                    }
                }
            }
        }
        return Ok(None);
    }

    Ok(None)
}

/// Multi-chunk parallel download using reqwest streaming
async fn run_multi_chunk(
    client: &Client,
    uri: &str,
    part_path: &Path,
    content_length: u64,
    split: usize,
    headers: &HeaderMap,
    total: Arc<AtomicU64>,
    completed: Arc<AtomicU64>,
    speed: Arc<AtomicU64>,
    cancel_token: CancellationToken,
    filename: &str,
    dir_path: &Path,
    global_limiter: Arc<SpeedLimiter>,
    task_limiter: Arc<SpeedLimiter>,
    expected_etag: Option<String>,
) -> Result<PathBuf, String> {
    total.store(content_length, Ordering::Relaxed);

    // Pre-allocate the output file
    {
        let file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(part_path)
            .map_err(|e| format!("Failed to create file: {e}"))?;
        file.set_len(content_length)
            .map_err(|e| format!("Failed to pre-allocate file: {e}"))?;
    }

    tracing::info!("Multi-chunk download: {split} chunks, {content_length} bytes total");

    // Calculate chunk boundaries
    let chunk_size = content_length / split as u64;
    let mut chunks: Vec<ChunkRange> = Vec::with_capacity(split);
    for i in 0..split {
        let start = i as u64 * chunk_size;
        let end = if i == split - 1 {
            content_length - 1
        } else {
            (i as u64 + 1) * chunk_size - 1
        };
        chunks.push(ChunkRange::new(start, end));
    }

    let file = tokio::fs::OpenOptions::new()
        .write(true)
        .open(part_path)
        .await
        .map_err(|e| format!("Failed to open file: {e}"))?;
    let file = Arc::new(Mutex::new(file));

    // Speed tracking
    let speed_cancel = cancel_token.clone();
    let speed_completed = completed.clone();
    let speed_val = speed.clone();
    let speed_total = total.clone();
    let speed_task = tokio::spawn(async move {
        run_speed_tracker(speed_completed, speed_val, speed_total, speed_cancel).await;
    });

    // Spawn all chunk downloads concurrently
    let mut futures = futures_util::stream::FuturesUnordered::new();
    for (i, chunk) in chunks.iter().enumerate() {
        let client = client.clone();
        let uri = uri.to_string();
        let headers = headers.clone();
        let file = file.clone();
        let completed = completed.clone();
        let cancel_token = cancel_token.clone();
        let chunk = *chunk;
        let gl = global_limiter.clone();
        let tl = task_limiter.clone();
        let etag = expected_etag.clone();

        futures.push(tokio::spawn(async move {
            download_chunk(&client, &uri, &headers, chunk, &file, &completed, cancel_token, i, &gl, &tl, etag.as_deref())
                .await
        }));
    }

    // Collect results
    let mut errors = Vec::new();
    while let Some(result) = futures.next().await {
        match result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => errors.push(e),
            Err(e) => errors.push(format!("Chunk task panicked: {e}")),
        }
    }

    // Stop speed tracker
    speed_task.abort();
    speed.store(0, Ordering::Relaxed);

    if !errors.is_empty() {
        if errors.iter().all(|e| e.contains("cancelled")) {
            return Err("Download cancelled".to_string());
        }
        let real_errors: Vec<&String> =
            errors.iter().filter(|e| !e.contains("cancelled")).collect();
        if !real_errors.is_empty() {
            return Err(real_errors
                .iter()
                .map(|e| e.as_str())
                .collect::<Vec<_>>()
                .join("; "));
        }
    }

    finalize_download(part_path, filename, dir_path)
}

/// Download a single chunk (byte range) using reqwest streaming with retry
async fn download_chunk(
    client: &Client,
    uri: &str,
    headers: &HeaderMap,
    chunk: ChunkRange,
    file: &Arc<Mutex<tokio::fs::File>>,
    completed: &Arc<AtomicU64>,
    cancel_token: CancellationToken,
    chunk_index: usize,
    global_limiter: &SpeedLimiter,
    task_limiter: &SpeedLimiter,
    expected_etag: Option<&str>,
) -> Result<(), String> {
    let mut bytes_written: u64 = 0;
    let mut retry_count: u32 = 0;

    loop {
        if cancel_token.is_cancelled() {
            return Err("Download cancelled".to_string());
        }

        let current_start = chunk.start + bytes_written;
        if current_start > chunk.end {
            return Ok(());
        }

        let current_range = ChunkRange::new(current_start, chunk.end);
        let result = download_chunk_stream(
            client,
            uri,
            headers,
            current_range,
            file,
            completed,
            &cancel_token,
            global_limiter,
            task_limiter,
            expected_etag,
        )
        .await;

        match result {
            Ok(written) => {
                bytes_written += written;
                if bytes_written >= chunk.len() {
                    return Ok(());
                }
                // Partial success, continue to download the rest of the chunk
                retry_count = 0;
            }
            Err(e) if e.contains("cancelled") => {
                return Err(e);
            }
            Err(e) => {
                retry_count += 1;
                if retry_count > CHUNK_MAX_RETRIES {
                    return Err(format!(
                        "Chunk {chunk_index} failed after {CHUNK_MAX_RETRIES} retries: {e}"
                    ));
                }
                tracing::warn!(
                    "Chunk {chunk_index} attempt {retry_count}/{CHUNK_MAX_RETRIES} failed: {e}, \
                     resuming from byte {}",
                    chunk.start + bytes_written
                );
                tokio::time::sleep(std::time::Duration::from_secs(retry_count as u64)).await;
            }
        }
    }
}

/// Stream a chunk range. Returns bytes successfully written to disk
async fn download_chunk_stream(
    client: &Client,
    uri: &str,
    headers: &HeaderMap,
    range: ChunkRange,
    file: &Arc<Mutex<tokio::fs::File>>,
    completed: &Arc<AtomicU64>,
    cancel_token: &CancellationToken,
    global_limiter: &SpeedLimiter,
    task_limiter: &SpeedLimiter,
    expected_etag: Option<&str>,
) -> Result<u64, String> {
    let mut req = client
        .get(uri)
        .headers(headers.clone())
        .header(RANGE, range.to_range_header_value());

    // If we have an ETag from the probe, send If-Match to ensure the file hasn't changed
    // If the ETag no longer matches, the server returns 412 Precondition Failed
    if let Some(etag) = expected_etag {
        if let Ok(v) = HeaderValue::from_str(etag) {
            req = req.header(IF_MATCH, v);
        }
    }

    let resp = req
        .send()
        .await
        .map_err(|e| format!("HTTP request failed: {e}"))?;

    let status = resp.status().as_u16();

    // For range requests, validate that the ETag still matches
    if let Some(expected) = expected_etag {
        if let Some(actual) = resp.headers().get(ETAG).and_then(|v| v.to_str().ok()) {
            if actual != expected {
                return Err("Server file changed (ETag mismatch), aborting download".to_string());
            }
        }
    }

    if status >= 400 {
        return Err(format!("HTTP error: {status}"));
    }

    let mut stream = resp.bytes_stream();
    let mut buf = Vec::with_capacity(CHUNK_BUF_CAPACITY);
    let mut total_written: u64 = 0;

    loop {
        tokio::select! {
            _ = cancel_token.cancelled() => {
                if !buf.is_empty() {
                    flush_buf_at(file, range.start + total_written, &buf).await?;
                }
                return Err("Download cancelled".to_string());
            }
            chunk = stream.next() => {
                match chunk {
                    Some(Ok(bytes)) => {
                        let len = bytes.len();

                        // Apply speed limits before accounting for the bytes
                        global_limiter.acquire(len).await;
                        task_limiter.acquire(len).await;

                        buf.extend_from_slice(&bytes);
                        completed.fetch_add(len as u64, Ordering::Relaxed);

                        if buf.len() >= CHUNK_BUF_CAPACITY {
                            let written = flush_buf_at(
                                file, range.start + total_written, &buf,
                            ).await?;
                            total_written += written as u64;
                            buf.clear();
                        }
                    }
                    Some(Err(e)) => {
                        if !buf.is_empty() {
                            flush_buf_at(
                                file, range.start + total_written, &buf,
                            ).await?;
                        }
                        return Err(format!("Stream error: {e}"));
                    }
                    None => {
                        if !buf.is_empty() {
                            let written = flush_buf_at(
                                file, range.start + total_written, &buf,
                            ).await?;
                            total_written += written as u64;
                        }
                        return Ok(total_written);
                    }
                }
            }
        }
    }
}

/// Flush buffer to file at offset
async fn flush_buf_at(
    file: &Arc<Mutex<tokio::fs::File>>,
    offset: u64,
    buf: &[u8],
) -> Result<usize, String> {
    let mut f = file.lock().await;
    f.seek(SeekFrom::Start(offset))
        .await
        .map_err(|e| format!("Seek failed: {e}"))?;
    f.write_all(buf)
        .await
        .map_err(|e| format!("Write failed: {e}"))?;
    Ok(buf.len())
}

/// Single-connection download.
/// Returns (final_path, last_modified_header_value).
async fn run_single_download(
    client: &Client,
    uri: &str,
    part_path: &Path,
    headers: &HeaderMap,
    total: Arc<AtomicU64>,
    completed: Arc<AtomicU64>,
    speed: Arc<AtomicU64>,
    cancelled: Arc<AtomicBool>,
    cancel_token: CancellationToken,
    filename: &str,
    dir_path: &Path,
    global_limiter: Arc<SpeedLimiter>,
    task_limiter: Arc<SpeedLimiter>,
) -> Result<(PathBuf, Option<String>), String> {
    let existing_size = if part_path.exists() {
        fs::metadata(part_path).map(|m| m.len()).unwrap_or(0)
    } else {
        0
    };

    completed.store(existing_size, Ordering::Relaxed);

    let mut req = client.get(uri).headers(headers.clone());
    if existing_size > 0 {
        req = req.header(RANGE, format!("bytes={existing_size}-"));
    }

    let resp = req.send().await.map_err(|e| {
        if cancelled.load(Ordering::Relaxed) {
            "Download cancelled".to_string()
        } else {
            format!("Download failed: {e}")
        }
    })?;

    let status = resp.status().as_u16();

    if status == 416 && existing_size > 0 {
        tracing::warn!("Got 416 with existing_size={existing_size}, deleting stale .part");
        let _ = fs::remove_file(part_path);
        return Err(format!("Download will retry: {STALE_PART_REMOVED}"));
    }

    if status >= 400 {
        return Err(format!("HTTP error: {status}"));
    }

    let resp_last_modified = resp
        .headers()
        .get(LAST_MODIFIED)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // Update total from Content-Length
    if let Some(cl) = resp.content_length() {
        if cl > 0 {
            total.store(existing_size + cl, Ordering::Relaxed);
        }
    }

    // Open file for appending
    let mut file = if existing_size > 0 {
        tokio::fs::OpenOptions::new()
            .append(true)
            .open(part_path)
            .await
            .map_err(|e| format!("Failed to open file for resume: {e}"))?
    } else {
        tokio::fs::File::create(part_path)
            .await
            .map_err(|e| format!("Failed to create file: {e}"))?
    };

    // Speed tracking
    let speed_cancel = cancel_token.clone();
    let speed_completed = completed.clone();
    let speed_val = speed.clone();
    let speed_total = total.clone();
    let speed_task = tokio::spawn(async move {
        run_speed_tracker(speed_completed, speed_val, speed_total, speed_cancel).await;
    });

    let mut stream = resp.bytes_stream();
    let mut buf = Vec::with_capacity(CHUNK_BUF_CAPACITY);

    let result = loop {
        tokio::select! {
            _ = cancel_token.cancelled() => {
                if !buf.is_empty() {
                    let _ = file.write_all(&buf).await;
                    let _ = file.flush().await;
                }
                break Err("Download cancelled".to_string());
            }
            chunk = stream.next() => {
                match chunk {
                    Some(Ok(bytes)) => {
                        let len = bytes.len();

                        // Apply speed limits before accounting for the bytes
                        global_limiter.acquire(len).await;
                        task_limiter.acquire(len).await;

                        buf.extend_from_slice(&bytes);
                        completed.fetch_add(len as u64, Ordering::Relaxed);

                        if buf.len() >= CHUNK_BUF_CAPACITY {
                            file.write_all(&buf).await
                                .map_err(|e| format!("Write failed: {e}"))?;
                            file.flush().await
                                .map_err(|e| format!("Flush failed: {e}"))?;
                            buf.clear();
                        }
                    }
                    Some(Err(e)) => {
                        if !buf.is_empty() {
                            let _ = file.write_all(&buf).await;
                            let _ = file.flush().await;
                        }
                        break Err(format!("Download failed: {e}"));
                    }
                    None => {
                        if !buf.is_empty() {
                            file.write_all(&buf).await
                                .map_err(|e| format!("Write failed: {e}"))?;
                        }
                        file.flush().await
                            .map_err(|e| format!("Flush failed: {e}"))?;
                        file.sync_all().await
                            .map_err(|e| format!("Sync failed: {e}"))?;
                        break Ok(());
                    }
                }
            }
        }
    };

    speed_task.abort();
    speed.store(0, Ordering::Relaxed);

    if let Err(e) = result {
        return Err(e);
    }
    let final_path = finalize_download(part_path, filename, dir_path)?;
    Ok((final_path, resp_last_modified))
}

/// Speed tracker that samples completed bytes every 500ms using EMA
async fn run_speed_tracker(
    completed: Arc<AtomicU64>,
    speed: Arc<AtomicU64>,
    total: Arc<AtomicU64>,
    cancel_token: CancellationToken,
) {
    let mut last_bytes = completed.load(Ordering::Relaxed);
    let mut last_time = tokio::time::Instant::now();
    let mut ema_speed: f64 = 0.0;
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(500));

    loop {
        interval.tick().await;
        if cancel_token.is_cancelled() {
            break;
        }

        let now = tokio::time::Instant::now();
        let elapsed = now.duration_since(last_time).as_secs_f64();
        let current = completed.load(Ordering::Relaxed);

        if elapsed > 0.0 {
            let delta = current.saturating_sub(last_bytes);
            let instant_speed = delta as f64 / elapsed;

            if ema_speed < 1.0 {
                ema_speed = instant_speed;
            } else {
                ema_speed = SPEED_EMA_ALPHA * instant_speed + (1.0 - SPEED_EMA_ALPHA) * ema_speed;
            }
            speed.store(ema_speed as u64, Ordering::Relaxed);
            last_bytes = current;
            last_time = now;
        }

        let t = total.load(Ordering::Relaxed);
        if t > 0 && current >= t {
            break;
        }
    }
    speed.store(0, Ordering::Relaxed);
}

/// Rename the .part file to the final filename
fn finalize_download(
    part_path: &Path,
    filename: &str,
    dir_path: &Path,
) -> Result<PathBuf, String> {
    let final_name = if filename.ends_with(PART_SUFFIX) {
        filename[..filename.len() - PART_SUFFIX.len()].to_string()
    } else {
        filename.to_string()
    };
    let final_path = dir_path.join(&final_name);
    if part_path != final_path {
        fs::rename(part_path, &final_path).map_err(|e| format!("Failed to rename: {e}"))?;
    }
    Ok(final_path)
}

/// Apply the remote server's Last-Modified time to the downloaded file.
/// `last_modified_str` is the raw HTTP Last-Modified header value (RFC 2822 / RFC 7231).
fn apply_remote_file_time(path: &Path, last_modified_str: &str) {
    if let Some(ft) = parse_http_date(last_modified_str) {
        if let Err(e) = filetime::set_file_mtime(path, ft) {
            tracing::warn!("Failed to set remote file time on {}: {e}", path.display());
        } else {
            tracing::info!("Set remote file time on {}: {last_modified_str}", path.display());
        }
    } else {
        tracing::warn!("Could not parse Last-Modified header: {last_modified_str}");
    }
}

/// Parse an HTTP date string into a `FileTime`.
/// Supports the three formats allowed by RFC 7231:
///   - `Sun, 06 Nov 1994 08:49:37 GMT` (preferred)
///   - `Sunday, 06-Nov-94 08:49:37 GMT`
///   - `Sun Nov  6 08:49:37 1994`
fn parse_http_date(s: &str) -> Option<filetime::FileTime> {
    // Try std::time parsing via httpdate-style manual parse
    let s = s.trim();
    // Use the simple approach: try to parse common HTTP date formats
    static MONTHS: &[&str] = &[
        "Jan", "Feb", "Mar", "Apr", "May", "Jun",
        "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];

    fn month_num(m: &str) -> Option<u32> {
        MONTHS.iter().position(|&name| name.eq_ignore_ascii_case(m)).map(|i| i as u32 + 1)
    }

    // Try RFC 7231 preferred format: "Sun, 06 Nov 1994 08:49:37 GMT"
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() == 6 && parts[5].eq_ignore_ascii_case("GMT") {
        let day: u32 = parts[1].trim_end_matches(',').parse().ok()?;
        let month = month_num(parts[2])?;
        let year: i64 = parts[3].parse().ok()?;
        let time_parts: Vec<&str> = parts[4].split(':').collect();
        if time_parts.len() == 3 {
            let hour: u32 = time_parts[0].parse().ok()?;
            let min: u32 = time_parts[1].parse().ok()?;
            let sec: u32 = time_parts[2].parse().ok()?;

            // Calculate seconds since epoch
            let days = days_from_civil(year, month, day)?;
            let secs = days as i64 * 86400 + hour as i64 * 3600 + min as i64 * 60 + sec as i64;
            if secs >= 0 {
                return Some(filetime::FileTime::from_unix_time(secs, 0));
            }
        }
    }

    None
}

/// Convert a civil date (year, month, day) to days since Unix epoch.
/// Algorithm from Howard Hinnant.
fn days_from_civil(y: i64, m: u32, d: u32) -> Option<i64> {
    if m < 1 || m > 12 || d < 1 || d > 31 {
        return None;
    }
    let y = if m <= 2 { y - 1 } else { y };
    let m = m as i64;
    let d = d as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe as i64 * 365 + yoe as i64 / 4 - yoe as i64 / 100 + doy;
    Some(era * 146097 + doe - 719468)
}

pub fn infer_filename_from_uri(uri: &str) -> String {
    let without_hash = uri.split('#').next().unwrap_or(uri);
    let without_query = without_hash.split('?').next().unwrap_or(without_hash);
    let candidate = without_query.rsplit('/').next().unwrap_or("").trim();
    if candidate.is_empty() || !candidate.contains('.') {
        "download".to_string()
    } else {
        url_decode(candidate)
    }
}

fn url_decode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                result.push((hi << 4 | lo) as char);
                i += 3;
                continue;
            }
        }
        result.push(bytes[i] as char);
        i += 1;
    }
    result
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
