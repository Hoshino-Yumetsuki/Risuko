use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use super::speed_limiter::parse_speed_limit;

pub fn is_youtube_uri(uri: &str) -> bool {
    let trimmed = uri.trim();
    if trimmed.is_empty() {
        return false;
    }

    if trimmed.starts_with("https://youtu.be/") || trimmed.starts_with("http://youtu.be/") {
        return true;
    }

    let parsed = match url::Url::parse(trimmed) {
        Ok(url) => url,
        Err(_) => return false,
    };

    let host = match parsed.host_str() {
        Some(h) => h.to_ascii_lowercase(),
        None => return false,
    };

    host == "youtube.com"
        || host == "www.youtube.com"
        || host == "m.youtube.com"
        || host == "music.youtube.com"
        || host.ends_with(".youtube.com")
        || host == "youtu.be"
}

pub async fn check_yt_dlp_available() -> Result<(), String> {
    let output = Command::new("yt-dlp")
        .arg("--version")
        .output()
        .await
        .map_err(|e| format!("yt-dlp is not available in PATH: {e}"))?;

    if output.status.success() {
        Ok(())
    } else {
        Err("yt-dlp is not available in PATH".to_string())
    }
}

fn parse_na_u64(s: &str) -> Option<u64> {
    if s == "NA" || s.is_empty() {
        return None;
    }
    // yt-dlp may emit floats (e.g. speed as 1234567.89)
    s.parse::<f64>().ok().map(|f| f.max(0.0) as u64)
}

fn min_nonzero_limit(lhs: u64, rhs: u64) -> u64 {
    match (lhs, rhs) {
        (0, 0) => 0,
        (0, value) | (value, 0) => value,
        (left, right) => left.min(right),
    }
}

fn option_non_empty_str<'a>(options: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    options
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn apply_yt_dlp_network_options(cmd: &mut Command, options: &Map<String, Value>) {
    if let Some(proxy) = option_non_empty_str(options, "all-proxy")
        .or_else(|| option_non_empty_str(options, "proxy"))
    {
        cmd.arg("--proxy").arg(proxy);
    }

    if let Some(user_agent) = option_non_empty_str(options, "user-agent") {
        cmd.arg("--user-agent").arg(user_agent);
    }

    if let Some(referer) = option_non_empty_str(options, "referer") {
        cmd.arg("--referer").arg(referer);
    }

    if let Some(cookies_file) = option_non_empty_str(options, "cookies") {
        cmd.arg("--cookies").arg(cookies_file);
    } else if let Some(cookie_header) = option_non_empty_str(options, "cookie") {
        cmd.arg("--add-header")
            .arg(format!("Cookie: {cookie_header}"));
    }
}

pub async fn run_youtube_download(
    url: &str,
    dir: &str,
    out: &str,
    options: &Map<String, Value>,
    global_rate_limit: u64,
    total: Arc<AtomicU64>,
    completed: Arc<AtomicU64>,
    speed: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
    connections: Arc<AtomicU32>,
    cancel_token: CancellationToken,
    // Sender for the resolved destination filename
    dest_tx: watch::Sender<String>,
) -> Result<PathBuf, String> {
    check_yt_dlp_available().await?;

    let mut cmd = Command::new("yt-dlp");
    cmd.arg("--newline")
        .arg("--no-color")
        // Force the web player client to avoid "not available on this app" errors
        .arg("--extractor-args")
        .arg("youtube:player_client=web,default")
        // Print the planned output path before each file download starts
        .arg("--print")
        .arg("before_dl:__YTDEST__%(filename)s")
        // Print the final moved path after download/merge completes
        .arg("--print")
        .arg("after_move:__YTPATH__%(filepath)s")
        // Progress template with raw byte values for precise tracking
        .arg("--progress")
        .arg("--progress-template")
        .arg("download:__YTPROG__%(progress.downloaded_bytes)s %(progress.total_bytes,progress.total_bytes_estimate)s %(progress.speed)s");

    apply_yt_dlp_network_options(&mut cmd, options);

    let format_opt = options
        .get("youtube-format")
        .or_else(|| options.get("yt-format"))
        .and_then(|v| v.as_str())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());
    if let Some(fmt) = format_opt {
        cmd.arg("--format").arg(fmt);
    }

    // yt-dlp runs out-of-process, so the shared Rust byte-bucket cannot
    // throttle it directly. Use the most restrictive configured launch-time
    // limit so YouTube downloads still respect the current app cap
    let task_rate_limit = options
        .get("max-download-limit")
        .map(parse_speed_limit)
        .unwrap_or(0);
    let rate_limit = min_nonzero_limit(task_rate_limit, global_rate_limit);
    if rate_limit > 0 {
        cmd.arg("--limit-rate").arg(rate_limit.to_string());
    }

    if !dir.trim().is_empty() {
        cmd.arg("-P").arg(dir);
    }

    if !out.trim().is_empty() {
        cmd.arg("-o").arg(out);
    }

    // Restrict to the single matched video; do not expand playlists
    cmd.arg("--no-playlist");

    cmd.arg(url);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    cmd.kill_on_drop(true);

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to start yt-dlp: {e}"))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "failed to capture yt-dlp stdout".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "failed to capture yt-dlp stderr".to_string())?;

    let stderr_task = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        let mut out = Vec::new();
        while let Ok(Some(line)) = lines.next_line().await {
            out.push(line);
            if out.len() > 40 {
                out.remove(0);
            }
        }
        out
    });

    let mut lines = BufReader::new(stdout).lines();
    let mut last_path: Option<PathBuf> = None;

    // Multi-stage progress accumulation
    // yt-dlp downloads video then audio separately when merging; each stage
    // starts its byte counter from zero. We detect a stage boundary when
    // downloaded_bytes drops below the last observed value and accumulate the
    // previous stage's total into base_bytes
    let mut base_bytes: u64 = 0;
    let mut last_stage_total: u64 = 0;
    let mut last_dl_bytes: u64 = 0;

    loop {
        tokio::select! {
            _ = cancel_token.cancelled() => {
                cancel.store(true, Ordering::Relaxed);
                let _ = child.kill().await;
                let _ = child.wait().await;
                let _ = stderr_task.await;
                return Err("cancelled".to_string());
            }
            next = lines.next_line() => {
                match next {
                    Ok(Some(line)) => {
                        let trimmed = line.trim();

                        if let Some(rest) = trimmed.strip_prefix("__YTDEST__") {
                            // Planned destination path, update task name immediately
                            let path_str = rest.trim().to_string();
                            if !path_str.is_empty() {
                                last_path = Some(PathBuf::from(&path_str));
                                let _ = dest_tx.send(path_str);
                            }
                        } else if let Some(rest) = trimmed.strip_prefix("__YTPATH__") {
                            // Final path after merge/move
                            let path_str = rest.trim();
                            if !path_str.is_empty() {
                                last_path = Some(PathBuf::from(path_str));
                            }
                        } else if let Some(rest) = trimmed.strip_prefix("__YTPROG__") {
                            // Raw progress: downloaded_bytes total_bytes_estimate speed
                            let parts: Vec<&str> = rest.split_ascii_whitespace().collect();
                            if parts.len() >= 2 {
                                let dl_bytes = parse_na_u64(parts[0]);
                                let total_bytes = parse_na_u64(parts[1]);
                                let speed_bytes = if parts.len() >= 3 {
                                    parse_na_u64(parts[2])
                                } else {
                                    None
                                };

                                if let Some(dl) = dl_bytes {
                                    // Detect new stage, bytes counter reset
                                    if dl < last_dl_bytes && last_dl_bytes > 65536 {
                                        base_bytes += last_stage_total;
                                    }
                                    last_dl_bytes = dl;
                                    if let Some(t) = total_bytes {
                                        last_stage_total = t;
                                        total.store(base_bytes + t, Ordering::Relaxed);
                                    }
                                    completed.store(base_bytes + dl, Ordering::Relaxed);
                                    connections.store(1, Ordering::Relaxed);
                                }
                                if let Some(s) = speed_bytes {
                                    speed.store(s, Ordering::Relaxed);
                                }
                            }
                        }
                        // Fallback: old-style "[download] Destination:" line in case
                        // the before_dl print isn't available in an older yt-dlp
                        else if let Some(rest) = trimmed.strip_prefix("[download] Destination: ") {
                            let path_str = rest.trim().to_string();
                            if !path_str.is_empty() {
                                last_path = Some(PathBuf::from(&path_str));
                                let _ = dest_tx.send(path_str);
                            }
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        let _ = child.kill().await;
                        let _ = child.wait().await;
                        let _ = stderr_task.await;
                        return Err(format!("failed reading yt-dlp output: {e}"));
                    }
                }
            }
        }
    }

    let status = child
        .wait()
        .await
        .map_err(|e| format!("failed waiting for yt-dlp: {e}"))?;

    let stderr_lines = stderr_task.await.unwrap_or_default();
    if !status.success() {
        let stderr_summary = stderr_lines
            .iter()
            .rev()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        if stderr_summary.is_empty() {
            return Err(format!("yt-dlp failed with exit code {:?}", status.code()));
        }
        return Err(stderr_summary);
    }

    let resolved = if let Some(path) = last_path {
        if path.is_absolute() || dir.trim().is_empty() {
            path
        } else {
            Path::new(dir).join(path)
        }
    } else if !out.trim().is_empty() {
        if Path::new(out).is_absolute() || dir.trim().is_empty() {
            PathBuf::from(out)
        } else {
            Path::new(dir).join(out)
        }
    } else {
        Path::new(dir).to_path_buf()
    };

    // Only sync completed to total if total is known; if yt-dlp never reported a
    // total size (all fields were NA), total stays 0 and we must not overwrite the
    // last valid completed counter with 0.
    let total_val = total.load(Ordering::Relaxed);
    if total_val > 0 {
        completed.store(total_val, Ordering::Relaxed);
    }
    speed.store(0, Ordering::Relaxed);
    Ok(resolved)
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct YouTubeFormat {
    pub format_id: String,
    pub ext: String,
    pub resolution: String,
    pub fps: Option<f64>,
    pub vcodec: String,
    pub acodec: String,
    pub filesize: Option<u64>,
    pub filesize_approx: Option<u64>,
    pub tbr: Option<f64>,
    pub abr: Option<f64>,
    pub vbr: Option<f64>,
    pub format_note: String,
    pub audio_only: bool,
    pub video_only: bool,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct YouTubeVideoInfo {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub duration: Option<f64>,
    pub uploader: Option<String>,
    pub upload_date: Option<String>,
    pub thumbnail: Option<String>,
    pub view_count: Option<u64>,
    pub channel: Option<String>,
    pub webpage_url: String,
    pub formats: Vec<YouTubeFormat>,
    pub is_playlist: bool,
    pub playlist_count: Option<u64>,
}

fn extract_format(obj: &serde_json::Value) -> Option<YouTubeFormat> {
    let format_id = obj.get("format_id")?.as_str()?.to_string();
    let ext = obj
        .get("ext")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let vcodec = obj
        .get("vcodec")
        .and_then(|v| v.as_str())
        .unwrap_or("none")
        .to_string();
    let acodec = obj
        .get("acodec")
        .and_then(|v| v.as_str())
        .unwrap_or("none")
        .to_string();
    let resolution = obj
        .get("resolution")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            let w = obj.get("width").and_then(|v| v.as_u64());
            let h = obj.get("height").and_then(|v| v.as_u64());
            match (w, h) {
                (Some(w), Some(h)) => format!("{w}x{h}"),
                _ => "audio only".to_string(),
            }
        });
    let fps = obj.get("fps").and_then(|v| v.as_f64());
    let filesize = obj.get("filesize").and_then(|v| v.as_u64());
    let filesize_approx = obj.get("filesize_approx").and_then(|v| v.as_u64());
    let tbr = obj.get("tbr").and_then(|v| v.as_f64());
    let abr = obj.get("abr").and_then(|v| v.as_f64());
    let vbr = obj.get("vbr").and_then(|v| v.as_f64());
    let format_note = obj
        .get("format_note")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let audio_only = vcodec == "none";
    let video_only = acodec == "none";

    Some(YouTubeFormat {
        format_id,
        ext,
        resolution,
        fps,
        vcodec,
        acodec,
        filesize,
        filesize_approx,
        tbr,
        abr,
        vbr,
        format_note,
        audio_only,
        video_only,
    })
}

pub async fn get_youtube_video_info(
    url: &str,
    options: &Map<String, Value>,
) -> Result<YouTubeVideoInfo, String> {
    check_yt_dlp_available().await?;

    let mut cmd = Command::new("yt-dlp");
    cmd.arg("--dump-json")
        .arg("--no-playlist")
        .arg("--flat-playlist")
        .arg("--extractor-args")
        .arg("youtube:player_client=web,default")
        .arg(url)
        .kill_on_drop(true);

    apply_yt_dlp_network_options(&mut cmd, options);

    let output = cmd
        .output()
        .await
        .map_err(|e| format!("failed to run yt-dlp: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let last = stderr
            .lines()
            .rev()
            .take(3)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        return Err(if last.is_empty() {
            format!("yt-dlp failed with exit code {:?}", output.status.code())
        } else {
            last
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let first_line = stdout
        .lines()
        .find(|l| !l.trim().is_empty())
        .ok_or_else(|| "yt-dlp returned no output".to_string())?;

    let json: serde_json::Value = serde_json::from_str(first_line)
        .map_err(|e| format!("failed to parse yt-dlp JSON: {e}"))?;

    let formats: Vec<YouTubeFormat> = json
        .get("formats")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(extract_format).collect())
        .unwrap_or_default();

    let is_playlist = json.get("_type").and_then(|v| v.as_str()) == Some("playlist");
    let playlist_count = json
        .get("playlist_count")
        .and_then(|v| v.as_u64())
        .or_else(|| {
            json.get("entries")
                .and_then(|v| v.as_array())
                .map(|a| a.len() as u64)
        });

    Ok(YouTubeVideoInfo {
        id: json
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        title: json
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or(url)
            .to_string(),
        description: json
            .get("description")
            .and_then(|v| v.as_str())
            .map(|s| s.chars().take(500).collect()),
        duration: json.get("duration").and_then(|v| v.as_f64()),
        uploader: json
            .get("uploader")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        upload_date: json
            .get("upload_date")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        thumbnail: json
            .get("thumbnail")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        view_count: json.get("view_count").and_then(|v| v.as_u64()),
        channel: json
            .get("channel")
            .or_else(|| json.get("uploader"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        webpage_url: json
            .get("webpage_url")
            .and_then(|v| v.as_str())
            .unwrap_or(url)
            .to_string(),
        formats,
        is_playlist,
        playlist_count,
    })
}
