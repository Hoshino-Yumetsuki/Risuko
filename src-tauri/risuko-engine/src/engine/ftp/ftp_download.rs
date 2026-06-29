use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use serde_json::{Map, Value};
use suppaftp::tokio::{AsyncFtpStream, AsyncRustlsFtpStream};
use tokio::io::AsyncReadExt as _;
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

use super::{FtpProtocol, FtpUri};
use crate::engine::speed_limiter::{SpeedEma, SpeedLimiter};

const PART_SUFFIX: &str = ".part";
const BUF_SIZE: usize = 64 * 1024;

/// Macro to perform the FTP/FTPS download loop on a connected + logged-in stream
/// Avoids duplicating the loop logic for `AsyncFtpStream` vs `AsyncRustlsFtpStream`
/// since `into_secure` changes the concrete stream type
macro_rules! ftp_transfer {
    ($ftp:expr, $parsed:expr, $part_path:expr, $file_size:expr,
     $total:expr, $completed:expr, $speed:expr, $cancelled:expr,
     $connections:expr, $cancel_token:expr, $global_limiter:expr, $task_limiter:expr) => {{
        $ftp.transfer_type(suppaftp::types::FileType::Binary)
            .await
            .map_err(|e| format!("Failed to set binary mode: {e}"))?;

        let remote_path = &$parsed.path;
        let remote_size = $ftp.size(remote_path).await.unwrap_or(0) as u64;
        if remote_size > 0 {
            $total.store(remote_size, Ordering::Relaxed);
        }

        $connections.store(1, Ordering::Relaxed);

        let existing_size = if $part_path.exists() {
            fs::metadata(&$part_path).map(|m| m.len()).unwrap_or(0)
        } else {
            0
        };

        let effective_size = if remote_size > 0 {
            remote_size
        } else {
            $file_size
        };
        // If .part already matches the remote size, skip transfer and let finalization rename it
        // Oversized .part files are stale and get recreated in the resume branch below
        if existing_size > 0 && effective_size > 0 && existing_size == effective_size {
            $completed.store(existing_size, Ordering::Relaxed);
            tracing::info!("FTP .part already complete ({existing_size} bytes), skipping transfer");
            let _ = $ftp.quit().await;
        } else {
            let resume_offset =
                if existing_size > 0 && effective_size > 0 && existing_size < effective_size {
                    // Avoid truncating the u64 resume offset on 32-bit targets
                    match usize::try_from(existing_size) {
                        Ok(off) => match $ftp.resume_transfer(off).await {
                            Ok(()) => {
                                $completed.store(existing_size, Ordering::Relaxed);
                                tracing::info!("Resuming FTP download from byte {existing_size}");
                                existing_size
                            }
                            Err(e) => {
                                tracing::warn!("FTP resume not supported: {e}");
                                0
                            }
                        },
                        Err(_) => {
                            tracing::warn!(
                                "FTP resume offset {existing_size} exceeds usize; \
                                 restarting download from scratch"
                            );
                            0
                        }
                    }
                } else {
                    0
                };

            let mut file = if resume_offset > 0 {
                tokio::fs::OpenOptions::new()
                    .write(true)
                    .append(true)
                    .open(&$part_path)
                    .await
                    .map_err(|e| format!("Failed to open part file: {e}"))?
            } else {
                tokio::fs::File::create(&$part_path)
                    .await
                    .map_err(|e| format!("Failed to create part file: {e}"))?
            };

            let mut data_stream = $ftp
                .retr_as_stream(remote_path)
                .await
                .map_err(|e| format!("FTP RETR failed: {e}"))?;

            let mut bytes_downloaded = resume_offset;
            let mut buf = vec![0u8; BUF_SIZE];
            let mut last_speed_time = Instant::now();
            let mut interval_bytes: u64 = 0;
            let mut ema = SpeedEma::new();

            loop {
                if $cancelled.load(Ordering::Relaxed) || $cancel_token.is_cancelled() {
                    return Err("Download cancelled".to_string());
                }

                let n = tokio::select! {
                    result = data_stream.read(&mut buf) => {
                        result.map_err(|e| format!("FTP read error: {e}"))?
                    }
                    _ = $cancel_token.cancelled() => {
                        return Err("Download cancelled".to_string());
                    }
                };

                if n == 0 {
                    break;
                }

                $global_limiter.acquire(n).await;
                $task_limiter.acquire(n).await;

                file.write_all(&buf[..n])
                    .await
                    .map_err(|e| format!("Failed to write: {e}"))?;

                bytes_downloaded += n as u64;
                $completed.store(bytes_downloaded, Ordering::Relaxed);
                interval_bytes += n as u64;

                let elapsed = last_speed_time.elapsed();
                if elapsed.as_millis() >= 500 {
                    $speed.store(
                        ema.update(interval_bytes, elapsed.as_secs_f64()),
                        Ordering::Relaxed,
                    );
                    interval_bytes = 0;
                    last_speed_time = Instant::now();
                }
            }

            file.flush()
                .await
                .map_err(|e| format!("Failed to flush: {e}"))?;
            drop(file);

            $ftp.finalize_retr_stream(data_stream)
                .await
                .map_err(|e| format!("FTP finalize failed: {e}"))?;

            let _ = $ftp.quit().await;
        }
        Ok::<(), String>(())
    }};
}

/// Run an FTP or FTPS download
#[allow(clippy::too_many_arguments)]
pub async fn run_ftp_ftps_download(
    parsed: &FtpUri,
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
    tracing::info!(
        "Starting FTP{} download: host={}, path={}",
        if parsed.protocol == FtpProtocol::Ftps {
            "S"
        } else {
            ""
        },
        parsed.host,
        parsed.path,
    );

    let dir_path = Path::new(dir);
    fs::create_dir_all(dir_path).map_err(|e| format!("Failed to create dir: {e}"))?;

    let filename = if out.is_empty() {
        basename_from_ftp_path(&parsed.path)
    } else {
        out.to_string()
    };

    let part_name = if filename.ends_with(PART_SUFFIX) {
        filename.clone()
    } else {
        format!("{filename}{PART_SUFFIX}")
    };
    let part_path = dir_path.join(&part_name);

    let user = parsed
        .user
        .clone()
        .or_else(|| option_str(options, "ftp-user"))
        .unwrap_or_else(|| "anonymous".to_string());
    let password = parsed
        .password
        .clone()
        .or_else(|| option_str(options, "ftp-passwd"))
        .unwrap_or_else(|| "risuko@".to_string());

    let addr = format!("{}:{}", parsed.host, parsed.port);
    let file_size = total.load(Ordering::Relaxed);

    if parsed.protocol == FtpProtocol::Ftps {
        // Match aria2's `--check-certificate=true` default
        // Only accept self-signed or invalid certs when `check-certificate=false`
        let verify_cert = option_bool(options, "check-certificate", true);
        if !verify_cert {
            tracing::warn!(
                "FTPS certificate verification disabled (check-certificate=false) for {}",
                parsed.host
            );
        }
        let connector = super::tls::ftps_tls_connector(!verify_cert);

        let mut ftp = if parsed.port == 990 {
            AsyncRustlsFtpStream::connect_secure_implicit(&addr, connector, &parsed.host)
                .await
                .map_err(|e| format!("FTPS implicit connect failed: {e}"))?
        } else {
            AsyncRustlsFtpStream::connect(&addr)
                .await
                .map_err(|e| format!("FTPS explicit connect failed: {e}"))?
                .into_secure(connector, &parsed.host)
                .await
                .map_err(|e| format!("FTPS AUTH TLS failed: {e}"))?
        };

        ftp.login(&user, &password)
            .await
            .map_err(|e| format!("FTP login failed: {e}"))?;

        ftp_transfer!(
            ftp,
            parsed,
            part_path,
            file_size,
            total,
            completed,
            speed,
            cancelled,
            connections,
            cancel_token,
            global_limiter,
            task_limiter
        )?;
    } else {
        let mut ftp = AsyncFtpStream::connect(&addr)
            .await
            .map_err(|e| format!("FTP connect failed: {e}"))?;

        ftp.login(&user, &password)
            .await
            .map_err(|e| format!("FTP login failed: {e}"))?;

        ftp_transfer!(
            ftp,
            parsed,
            part_path,
            file_size,
            total,
            completed,
            speed,
            cancelled,
            connections,
            cancel_token,
            global_limiter,
            task_limiter
        )?;
    }

    // Final stats
    let bytes_done = completed.load(Ordering::Relaxed);
    if total.load(Ordering::Relaxed) == 0 {
        total.store(bytes_done, Ordering::Relaxed);
    }
    speed.store(0, Ordering::Relaxed);
    connections.store(0, Ordering::Relaxed);

    let final_path = finalize_download(&part_path, &filename, dir_path)?;
    tracing::info!("FTP download complete: {}", final_path.display());
    Ok(final_path)
}

pub(super) fn finalize_download(
    part_path: &Path,
    filename: &str,
    dir_path: &Path,
) -> Result<PathBuf, String> {
    let final_name = if let Some(stripped) = filename.strip_suffix(PART_SUFFIX) {
        stripped.to_string()
    } else {
        filename.to_string()
    };
    let final_path = dedup_path(dir_path, &final_name);
    if part_path != final_path {
        fs::rename(part_path, &final_path).map_err(|e| format!("Failed to rename: {e}"))?;
    }
    Ok(final_path)
}

/// If `dir/name` already exists, return `dir/stem.1.ext`, `dir/stem.2.ext`, etc.
pub(super) fn dedup_path(dir: &Path, name: &str) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }

    let (stem, ext) = match name.rfind('.') {
        Some(dot) if dot > 0 => (&name[..dot], &name[dot..]), // "file.txt" -> ("file", ".txt")
        _ => (name, ""),                                      // "noext" -> ("noext", "")
    };

    for n in 1u32.. {
        let numbered = if ext.is_empty() {
            format!("{stem}.{n}")
        } else {
            format!("{stem}.{n}{ext}")
        };
        let path = dir.join(&numbered);
        if !path.exists() {
            return path;
        }
    }
    // Unreachable in practice
    candidate
}

fn option_str(options: &Map<String, Value>, key: &str) -> Option<String> {
    options
        .get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

/// Read a boolean option from JSON bools or common string forms
///
/// Returns `default` when absent or unrecognized
fn option_bool(options: &Map<String, Value>, key: &str, default: bool) -> bool {
    match options.get(key) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => match s.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => true,
            "false" | "0" | "no" | "off" => false,
            _ => default,
        },
        _ => default,
    }
}

/// Derive a download filename from an already-parsed FTP path
fn basename_from_ftp_path(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(idx) if !trimmed[idx + 1..].is_empty() => trimmed[idx + 1..].to_string(),
        _ => "download".to_string(),
    }
}
