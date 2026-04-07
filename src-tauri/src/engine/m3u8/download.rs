use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use serde_json::{Map, Value};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

use super::parser::{self, ParsedPlaylist, Variant};
use super::segment;
use crate::engine::speed_limiter::SpeedLimiter;

/// Run an M3U8/HLS download 
/// Main entry point called from manager.rs
/// Returns the final output file path on success.
#[allow(clippy::too_many_arguments)]
pub async fn run_m3u8_download(
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

    check_cancelled(&cancelled, &cancel_token)?;

    let total_segments = segments.len();
    tracing::info!(
        "[m3u8] Downloading {total_segments} segments from {}",
        media_playlist_url
    );

    let split = options
        .get("split")
        .and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
        .unwrap_or(5)
        .max(1) as usize;

    // Create temp dir for segments
    let filename = if out.is_empty() {
        infer_filename_from_uri(uri)
    } else {
        out.to_string()
    };
    let temp_dir_name = format!(".m3u8_{}", sanitize_filename(&filename));
    let temp_dir = dir_path.join(&temp_dir_name);

    // Download all segments (speed tracker runs alongside)
    let speed_completed = completed.clone();
    let speed_val = speed.clone();
    let speed_cancel = cancel_token.clone();
    let speed_tracker = tokio::spawn(async move {
        run_speed_tracker(speed_completed, speed_val, speed_cancel).await;
    });

    let (seg_paths, progress) = segment::download_segments(
        &segments,
        media_sequence,
        &temp_dir,
        &client,
        total.clone(),
        completed.clone(),
        connections.clone(),
        cancelled.clone(),
        cancel_token.clone(),
        global_limiter,
        task_limiter,
        split,
    )
    .await?;

    // Stop speed tracker
    speed.store(0, Ordering::Relaxed);
    speed_tracker.abort();

    check_cancelled(&cancelled, &cancel_token)?;

    // Concatenate segments into final output
    let ts_path = dir_path.join(&filename);
    concatenate_segments(&seg_paths, &ts_path).await?;

    // Set final byte-accurate total from the output file
    if let Ok(meta) = tokio::fs::metadata(&ts_path).await {
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
        match remux_to_mp4(&ts_path).await {
            Ok(mp4_path) => {
                // Remove the .ts intermediate
                let _ = tokio::fs::remove_file(&ts_path).await;
                mp4_path
            }
            Err(e) => {
                tracing::warn!("[m3u8] ffmpeg remux failed, keeping .ts output: {e}");
                ts_path // fall back to .ts
            }
        }
    } else {
        ts_path
    };

    // Cleanup temp dir and progress
    progress.cleanup();
    let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    speed.store(0, Ordering::Relaxed);

    tracing::info!(
        "[m3u8] Download complete: {}",
        final_path.display()
    );
    Ok(final_path)
}

fn check_cancelled(
    cancelled: &AtomicBool,
    cancel_token: &CancellationToken,
) -> Result<(), String> {
    if cancelled.load(Ordering::Relaxed) || cancel_token.is_cancelled() {
        return Err("cancelled".to_string());
    }
    Ok(())
}

const SPEED_EMA_ALPHA: f64 = 0.3;

async fn run_speed_tracker(
    completed: Arc<AtomicU64>,
    speed: Arc<AtomicU64>,
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

fn build_client(options: &Map<String, Value>) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder();

    if let Some(ua) = options.get("user-agent").and_then(|v| v.as_str()) {
        builder = builder.user_agent(ua);
    } else {
        builder = builder.user_agent("Mozilla/5.0");
    }

    if let Some(proxy_url) = options.get("all-proxy").and_then(|v| v.as_str()) {
        if !proxy_url.is_empty() {
            let proxy =
                reqwest::Proxy::all(proxy_url).map_err(|e| format!("Invalid proxy: {e}"))?;
            builder = builder.proxy(proxy);
        }
    }

    builder.build().map_err(|e| format!("Failed to build HTTP client: {e}"))
}

fn infer_filename_from_uri(uri: &str) -> String {
    let path = uri.split('?').next().unwrap_or(uri);
    let path = path.split('#').next().unwrap_or(path);
    let name = path.rsplit('/').next().unwrap_or("download");

    // Replace .m3u8 extension with .ts
    if let Some(stem) = name.strip_suffix(".m3u8").or_else(|| name.strip_suffix(".m3u")) {
        format!("{stem}.ts")
    } else if name.is_empty() {
        "download.ts".to_string()
    } else {
        format!("{name}.ts")
    }
}

fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' })
        .collect()
}

/// Concatenate segment files into a single output file
async fn concatenate_segments(
    segment_paths: &[PathBuf],
    output_path: &Path,
) -> Result<(), String> {
    let mut output = tokio::fs::File::create(output_path)
        .await
        .map_err(|e| format!("Failed to create output file: {e}"))?;

    for path in segment_paths {
        if !path.exists() {
            return Err(format!("Missing segment file: {}", path.display()));
        }
        let data = tokio::fs::read(path)
            .await
            .map_err(|e| format!("Failed to read segment {}: {e}", path.display()))?;

        output
            .write_all(&data)
            .await
            .map_err(|e| format!("Failed to write to output: {e}"))?;
    }

    output
        .flush()
        .await
        .map_err(|e| format!("Failed to flush output: {e}"))?;

    Ok(())
}

/// Attempt to remux .ts to .mp4 using system ffmpeg
async fn remux_to_mp4(ts_path: &Path) -> Result<PathBuf, String> {
    let mp4_path = ts_path.with_extension("mp4");

    // Check ffmpeg availability
    let ffmpeg_check = tokio::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
        .await;

    if ffmpeg_check.is_err() {
        return Err("ffmpeg not found on system PATH".to_string());
    }

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
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("ffmpeg remux failed: {stderr}"));
    }

    Ok(mp4_path)
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
    fn test_sanitize_filename() {
        assert_eq!(sanitize_filename("hello world.ts"), "hello_world.ts");
        assert_eq!(sanitize_filename("video/name:1.ts"), "video_name_1.ts");
        assert_eq!(sanitize_filename("normal-file_01.ts"), "normal-file_01.ts");
    }
}
