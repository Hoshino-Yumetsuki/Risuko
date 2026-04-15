use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use futures_util::AsyncReadExt;
use serde_json::{Map, Value};
use suppaftp::{AsyncFtpStream, AsyncRustlsConnector, AsyncRustlsFtpStream};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

use super::{FtpProtocol, FtpUri};
use crate::engine::speed_limiter::SpeedLimiter;

const PART_SUFFIX: &str = ".part";
const BUF_SIZE: usize = 64 * 1024;
const SPEED_EMA_ALPHA: f64 = 0.3;

/// TLS certificate verifier that accepts any certificate
/// Used for FTPS servers with self-signed certificates
#[derive(Debug)]
struct AcceptAnyCert;

impl rustls::client::danger::ServerCertVerifier for AcceptAnyCert {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

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
        let resume_offset =
            if existing_size > 0 && (effective_size == 0 || existing_size < effective_size) {
                match $ftp.resume_transfer(existing_size as usize).await {
                    Ok(()) => {
                        $completed.store(existing_size, Ordering::Relaxed);
                        tracing::info!("Resuming FTP download from byte {existing_size}");
                        existing_size
                    }
                    Err(e) => {
                        tracing::warn!("FTP resume not supported: {e}");
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
        let mut ema_speed: f64 = 0.0;

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
                let secs = elapsed.as_secs_f64();
                let instant_speed = interval_bytes as f64 / secs;
                ema_speed = SPEED_EMA_ALPHA * instant_speed + (1.0 - SPEED_EMA_ALPHA) * ema_speed;
                $speed.store(ema_speed as u64, Ordering::Relaxed);
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
        super::infer_filename_from_ftp_uri(&format!("ftp://{}{}", parsed.host, parsed.path))
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
        let rustls_config = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAnyCert))
            .with_no_client_auth();
        let connector =
            AsyncRustlsConnector::from(futures_rustls::TlsConnector::from(Arc::new(rustls_config)));

        // Use implicit TLS connection for FTPS
        let mut ftp = AsyncRustlsFtpStream::connect_secure_implicit(&addr, connector, &parsed.host)
            .await
            .map_err(|e| format!("FTPS connect failed: {e}"))?;

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
    let final_name = if filename.ends_with(PART_SUFFIX) {
        filename[..filename.len() - PART_SUFFIX.len()].to_string()
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
