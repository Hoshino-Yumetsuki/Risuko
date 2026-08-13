use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

use super::parser::{self, ParsedPlaylist, Variant};
use super::segment;
use crate::engine::speed_limiter::{SpeedEma, SpeedLimiter};

static TEMP_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Run an M3U8/HLS download Main entry point called from manager.rs Returns the final output file path on success
#[allow(clippy::too_many_arguments)]
pub async fn run_m3u8_download(
    uri: &str,
    dir: &str,
    out: &str,
    options: &Map<String, Value>,
    total: Arc<AtomicU64>,
    completed: Arc<AtomicU64>,
    speed: Arc<AtomicU64>,
    connections: Arc<AtomicU32>,
    cancel_token: CancellationToken,
    global_limiter: Arc<SpeedLimiter>,
    task_limiter: Arc<SpeedLimiter>,
) -> Result<PathBuf, String> {
    tracing::info!("[m3u8] Starting download: uri={uri}, dir={dir}, out={out}");

    let dir_path = Path::new(dir);
    std::fs::create_dir_all(dir_path).map_err(|e| format!("Failed to create dir: {e}"))?;

    let client = build_client(options)?;

    // Fetch and parse the playlist
    let playlist = parser::fetch_and_parse_playlist(uri, &client).await?;

    // If master playlist, select a variant
    let (media_playlist_url, media_playlist) = match playlist {
        ParsedPlaylist::Master { variants } => {
            if variants.is_empty() {
                return Err("Master playlist has no variants".to_string());
            }
            let variant = select_variant(&variants, options);
            tracing::info!(
                "[m3u8] Selected variant: bandwidth={}, url={}",
                variant.bandwidth,
                variant.url
            );
            let media = parser::fetch_and_parse_playlist(&variant.url, &client).await?;
            (variant.url.clone(), media)
        }
        media @ ParsedPlaylist::Media { .. } => (uri.to_string(), media),
    };

    let ParsedPlaylist::Media {
        segments,
        media_sequence,
        end_list,
        total_duration: _,
    } = media_playlist
    else {
        return Err("Expected media playlist after variant resolution".to_string());
    };

    // Reject live streams
    if !end_list {
        return Err("Live streams (no #EXT-X-ENDLIST) are not supported".to_string());
    }

    if segments.is_empty() {
        return Err("Media playlist has no segments".to_string());
    }

    check_cancelled(&cancel_token)?;

    let total_segments = segments.len();
    tracing::info!(
        "[m3u8] Downloading {total_segments} segments from {}",
        media_playlist_url
    );

    let split = options
        .get("split")
        .and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        })
        .unwrap_or(5)
        .max(1) as usize;

    // Create temp dir for segments
    let filename = if out.is_empty() {
        infer_filename_from_uri(uri)
    } else {
        out.to_string()
    };
    let temp_dir_name = temp_dir_name_for(&filename);
    let temp_dir = dir_path.join(&temp_dir_name);

    // Download all segments (speed tracker runs alongside)
    let speed_completed = completed.clone();
    let speed_val = speed.clone();
    let speed_cancel = cancel_token.clone();
    let speed_tracker = tokio::spawn(async move {
        run_speed_tracker(speed_completed, speed_val, speed_cancel).await;
    });

    let segments_result = segment::download_segments(
        &segments,
        media_sequence,
        &temp_dir,
        &client,
        total.clone(),
        completed.clone(),
        connections.clone(),
        cancel_token.clone(),
        global_limiter,
        task_limiter,
        split,
    )
    .await;

    let (seg_paths, progress) = match segments_result {
        Ok(result) => result,
        Err(e) => {
            cancel_token.cancel();
            speed.store(0, Ordering::Relaxed);
            speed_tracker.abort();
            return Err(e);
        }
    };

    // Stop speed tracker
    speed.store(0, Ordering::Relaxed);
    speed_tracker.abort();

    check_cancelled(&cancel_token)?;

    // Concatenate segments into final output
    let final_ts_path = concatenate_segments_unique(&seg_paths, dir_path, &filename).await?;

    // Set final byte-accurate total from the output file
    if let Ok(meta) = tokio::fs::metadata(&final_ts_path).await {
        let file_size = meta.len();
        total.store(file_size, Ordering::Relaxed);
        completed.store(file_size, Ordering::Relaxed);
    }

    // Attempt ffmpeg remux if requested
    let output_format = options
        .get("m3u8-output-format")
        .and_then(|v| v.as_str())
        .unwrap_or("ts");

    let final_path = if output_format == "mp4" {
        match remux_to_mp4(&final_ts_path).await {
            Ok(mp4_path) => {
                // Remove the .ts intermediate
                let _ = tokio::fs::remove_file(&final_ts_path).await;
                mp4_path
            }
            Err(e) => {
                tracing::warn!("[m3u8] ffmpeg remux failed, keeping .ts output: {e}");
                final_ts_path // fall back to .ts
            }
        }
    } else {
        final_ts_path
    };

    // Cleanup temp dir and progress
    progress.cleanup();
    let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    speed.store(0, Ordering::Relaxed);

    tracing::info!("[m3u8] Download complete: {}", final_path.display());
    Ok(final_path)
}

fn check_cancelled(cancel_token: &CancellationToken) -> Result<(), String> {
    if cancel_token.is_cancelled() {
        return Err("cancelled".to_string());
    }
    Ok(())
}

async fn run_speed_tracker(
    completed: Arc<AtomicU64>,
    speed: Arc<AtomicU64>,
    cancel_token: CancellationToken,
) {
    let mut last_bytes = completed.load(Ordering::Relaxed);
    let mut last_time = tokio::time::Instant::now();
    let mut ema = SpeedEma::new();
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
            speed.store(ema.update(delta, elapsed), Ordering::Relaxed);
            last_bytes = current;
            last_time = now;
        }
    }
}

/// Select the best variant based on options or default to highest bandwidth
fn select_variant<'a>(variants: &'a [Variant], options: &Map<String, Value>) -> &'a Variant {
    // Check if a specific variant URL was chosen by the frontend
    if let Some(chosen_url) = options.get("m3u8-variant-url").and_then(|v| v.as_str()) {
        if let Some(v) = variants.iter().find(|v| v.url == chosen_url) {
            return v;
        }
    }

    // Default: highest bandwidth
    variants
        .iter()
        .max_by_key(|v| v.bandwidth)
        .unwrap_or(&variants[0])
}

fn build_client(options: &Map<String, Value>) -> Result<risuko_http::Client, String> {
    let mut builder = risuko_http::Client::builder();

    if let Some(ua) = options.get("user-agent").and_then(|v| v.as_str()) {
        builder = builder.user_agent(ua);
    } else {
        builder = builder.user_agent("Mozilla/5.0");
    }

    if let Some(proxy_url) = options.get("all-proxy").and_then(|v| v.as_str()) {
        if !proxy_url.is_empty() {
            let proxy =
                risuko_http::Proxy::all(proxy_url).map_err(|e| format!("Invalid proxy: {e}"))?;
            builder = builder.proxy(proxy);
        }
    }

    if let Some(no_proxy) = options
        .get("no-proxy")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let matcher = risuko_http::NoProxy::parse(no_proxy);
        if !matcher.is_empty() {
            builder = builder.no_proxy(matcher);
        }
    }

    builder
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {e}"))
}

fn infer_filename_from_uri(uri: &str) -> String {
    let path = uri.split('?').next().unwrap_or(uri);
    let path = path.split('#').next().unwrap_or(path);
    let name = path.rsplit('/').next().unwrap_or("download");

    // Replace .m3u8 extension with .ts
    if let Some(stem) = name
        .strip_suffix(".m3u8")
        .or_else(|| name.strip_suffix(".m3u"))
    {
        format!("{stem}.ts")
    } else if name.is_empty() {
        "download.ts".to_string()
    } else {
        format!("{name}.ts")
    }
}

fn temp_dir_name_for(filename: &str) -> String {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let counter = TEMP_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!(
        ".m3u8_{}_{}_{}_{}",
        crate::engine::util::safe_filename(filename, "download"),
        std::process::id(),
        nonce,
        counter
    )
}

fn final_path_candidate(dir: &Path, filename: &str, n: u32) -> PathBuf {
    let sanitized = crate::engine::util::safe_filename(filename, "download");
    if n == 0 {
        return dir.join(sanitized);
    }

    let (stem, ext) = match sanitized.rfind('.') {
        Some(dot) if dot > 0 => (&sanitized[..dot], &sanitized[dot..]),
        _ => (sanitized.as_str(), ""),
    };

    let numbered = if ext.is_empty() {
        format!("{stem}.{n}")
    } else {
        format!("{stem}.{n}{ext}")
    };
    dir.join(numbered)
}

/// Concatenate segment files into a single output file
async fn concatenate_segments_unique(
    segment_paths: &[PathBuf],
    output_dir: &Path,
    filename: &str,
) -> Result<PathBuf, String> {
    let output_path = reserve_unique_output_path(output_dir, filename).await?;
    if let Err(e) = concatenate_segments(segment_paths, &output_path).await {
        let _ = tokio::fs::remove_file(&output_path).await;
        return Err(e);
    }
    Ok(output_path)
}

async fn concatenate_segments(segment_paths: &[PathBuf], output_path: &Path) -> Result<(), String> {
    let mut output = tokio::fs::File::create(output_path)
        .await
        .map_err(|e| format!("Failed to create output file: {e}"))?;

    for path in segment_paths {
        let mut seg_file = tokio::fs::File::open(path)
            .await
            .map_err(|e| format!("Failed to open segment {}: {e}", path.display()))?;
        tokio::io::copy(&mut seg_file, &mut output)
            .await
            .map_err(|e| format!("Failed to write to output: {e}"))?;
    }

    output
        .flush()
        .await
        .map_err(|e| format!("Failed to flush output: {e}"))
}

/// Attempt to remux .ts to .mp4 using system ffmpeg
async fn remux_to_mp4(ts_path: &Path) -> Result<PathBuf, String> {
    let parent = ts_path
        .parent()
        .ok_or_else(|| "M3U8 output path has no parent directory".to_string())?;
    let mp4_name = ts_path
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| {
            let mut name = name.to_string();
            if let Some(dot) = name.rfind('.') {
                name.replace_range(dot.., ".mp4");
                name
            } else {
                format!("{name}.mp4")
            }
        })
        .ok_or_else(|| "M3U8 output filename is not valid UTF-8".to_string())?;
    let mp4_path = reserve_unique_output_path(parent, &mp4_name).await?;

    let output = tokio::process::Command::new("ffmpeg")
        .arg("-i")
        .arg(ts_path)
        .arg("-c")
        .arg("copy")
        .arg("-movflags")
        .arg("+faststart")
        .arg("-y")
        .arg(&mp4_path)
        .output()
        .await
        .map_err(|e| format!("ffmpeg execution failed: {e}"))?;

    if !output.status.success() {
        let _ = tokio::fs::remove_file(&mp4_path).await;
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("ffmpeg remux failed: {stderr}"));
    }

    Ok(mp4_path)
}

async fn reserve_unique_output_path(dir: &Path, filename: &str) -> Result<PathBuf, String> {
    for n in 0u32.. {
        let path = final_path_candidate(dir, filename, n);
        match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .await
        {
            Ok(_) => return Ok(path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("Failed to reserve output file: {e}")),
        }
    }

    Err("Failed to reserve output filename".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_infer_filename_from_uri() {
        assert_eq!(
            infer_filename_from_uri("https://example.com/video.m3u8"),
            "video.ts"
        );
        assert_eq!(
            infer_filename_from_uri("https://example.com/live/stream.m3u8?token=abc"),
            "stream.ts"
        );
        assert_eq!(
            infer_filename_from_uri("https://example.com/path/"),
            "download.ts"
        );
        assert_eq!(
            infer_filename_from_uri("https://example.com/video.m3u"),
            "video.ts"
        );
    }

    #[test]
    fn temp_dir_names_include_unique_suffix() {
        let a = temp_dir_name_for("video.ts");
        let b = temp_dir_name_for("video.ts");
        assert!(a.starts_with(".m3u8_video.ts_"));
        assert_ne!(a, b);
    }

    #[test]
    fn final_path_candidate_deduplicates_stem_and_extension() {
        let dir = Path::new("/downloads");
        assert_eq!(
            final_path_candidate(dir, "video.ts", 0),
            dir.join("video.ts")
        );
        assert_eq!(
            final_path_candidate(dir, "video.ts", 1),
            dir.join("video.1.ts")
        );
        assert_eq!(final_path_candidate(dir, "video", 2), dir.join("video.2"));
    }
}
