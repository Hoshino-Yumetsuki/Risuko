use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use russh::client;
use russh_keys::key;
use russh_sftp::client::SftpSession;
use serde_json::{Map, Value};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

use super::FtpUri;
use crate::engine::speed_limiter::SpeedLimiter;

const PART_SUFFIX: &str = ".part";
const BUF_SIZE: usize = 64 * 1024;
const SPEED_EMA_ALPHA: f64 = 0.3;

/// SSH client handler that accepts all host keys
/// TODO: TOFU verification: v0.1.1
struct SshHandler;

#[async_trait]
impl client::Handler for SshHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &key::PublicKey,
    ) -> Result<bool, Self::Error> {
        // Accept all host keys for now
        Ok(true)
    }
}

/// Run an SFTP download
#[allow(clippy::too_many_arguments)]
pub async fn run_sftp_download(
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
        "Starting SFTP download: host={}, path={}",
        parsed.host,
        parsed.path,
    );

    let dir_path = Path::new(dir);
    fs::create_dir_all(dir_path).map_err(|e| format!("Failed to create dir: {e}"))?;

    let filename = if out.is_empty() {
        super::infer_filename_from_ftp_uri(&format!("sftp://{}{}", parsed.host, parsed.path))
    } else {
        out.to_string()
    };

    let part_name = if filename.ends_with(PART_SUFFIX) {
        filename.clone()
    } else {
        format!("{filename}{PART_SUFFIX}")
    };
    let part_path = dir_path.join(&part_name);

    // Resolve credentials
    let user = parsed
        .user
        .clone()
        .or_else(|| option_str(options, "sftp-user"))
        .or_else(|| option_str(options, "ftp-user"))
        .unwrap_or_else(|| "root".to_string());

    let password = parsed
        .password
        .clone()
        .or_else(|| option_str(options, "sftp-passwd"))
        .or_else(|| option_str(options, "ftp-passwd"));

    let private_key_source = option_str(options, "sftp-private-key");
    let key_passphrase = option_str(options, "sftp-private-key-passphrase");

    // Connect via SSH
    let config = Arc::new(client::Config {
        ..Default::default()
    });

    let addr = format!("{}:{}", parsed.host, parsed.port);
    let mut session = client::connect(config, &addr, SshHandler)
        .await
        .map_err(|e| format!("SSH connect failed: {e}"))?;

    // Authenticate
    let authenticated = try_authenticate(
        &mut session,
        &user,
        password.as_deref(),
        private_key_source.as_deref(),
        key_passphrase.as_deref(),
    )
    .await?;

    if !authenticated {
        return Err("SSH authentication failed: no valid credentials".to_string());
    }

    // Open SFTP channel
    let channel = session
        .channel_open_session()
        .await
        .map_err(|e| format!("SSH channel open failed: {e}"))?;

    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|e| format!("SFTP subsystem request failed: {e}"))?;

    let sftp = SftpSession::new(channel.into_stream())
        .await
        .map_err(|e| format!("SFTP session init failed: {e}"))?;

    connections.store(1, Ordering::Relaxed);

    // Stat remote file for size
    let remote_path = &parsed.path;
    let file_size = match sftp.metadata(remote_path).await {
        Ok(attrs) => attrs.size.unwrap_or(0),
        Err(e) => {
            tracing::warn!("SFTP stat failed (continuing without size): {e}");
            0
        }
    };
    if file_size > 0 {
        total.store(file_size, Ordering::Relaxed);
    }

    // Check existing partial download
    let existing_size = if part_path.exists() {
        fs::metadata(&part_path).map(|m| m.len()).unwrap_or(0)
    } else {
        0
    };

    let resume_offset = if existing_size > 0 && file_size > 0 && existing_size < file_size {
        existing_size
    } else {
        0
    };

    if resume_offset > 0 {
        completed.store(resume_offset, Ordering::Relaxed);
        tracing::info!("Resuming SFTP download from byte {resume_offset}");
    }

    // Open remote file for reading
    let mut remote_file = sftp
        .open(remote_path)
        .await
        .map_err(|e| format!("SFTP open failed: {e}"))?;

    // Seek to resume offset if needed
    if resume_offset > 0 {
        use tokio::io::AsyncSeekExt;
        remote_file
            .seek(std::io::SeekFrom::Start(resume_offset))
            .await
            .map_err(|e| format!("SFTP seek failed: {e}"))?;
    }

    // Open local file
    let mut local_file = if resume_offset > 0 {
        tokio::fs::OpenOptions::new()
            .write(true)
            .append(true)
            .open(&part_path)
            .await
            .map_err(|e| format!("Failed to open part file: {e}"))?
    } else {
        tokio::fs::File::create(&part_path)
            .await
            .map_err(|e| format!("Failed to create part file: {e}"))?
    };

    // Download loop
    let mut bytes_downloaded = resume_offset;
    let mut buf = vec![0u8; BUF_SIZE];
    let mut last_speed_time = Instant::now();
    let mut interval_bytes: u64 = 0;
    let mut ema_speed: f64 = 0.0;

    use tokio::io::AsyncReadExt;

    loop {
        if cancelled.load(Ordering::Relaxed) || cancel_token.is_cancelled() {
            return Err("Download cancelled".to_string());
        }

        let n = tokio::select! {
            result = remote_file.read(&mut buf) => {
                result.map_err(|e| format!("SFTP read error: {e}"))?
            }
            _ = cancel_token.cancelled() => {
                return Err("Download cancelled".to_string());
            }
        };

        if n == 0 {
            break;
        }

        // Apply speed limiting
        global_limiter.acquire(n).await;
        task_limiter.acquire(n).await;

        local_file
            .write_all(&buf[..n])
            .await
            .map_err(|e| format!("Failed to write: {e}"))?;

        bytes_downloaded += n as u64;
        completed.store(bytes_downloaded, Ordering::Relaxed);
        interval_bytes += n as u64;

        // Update speed EMA every 500ms
        let elapsed = last_speed_time.elapsed();
        if elapsed.as_millis() >= 500 {
            let secs = elapsed.as_secs_f64();
            let instant_speed = interval_bytes as f64 / secs;
            ema_speed = SPEED_EMA_ALPHA * instant_speed + (1.0 - SPEED_EMA_ALPHA) * ema_speed;
            speed.store(ema_speed as u64, Ordering::Relaxed);
            interval_bytes = 0;
            last_speed_time = Instant::now();
        }
    }

    local_file
        .flush()
        .await
        .map_err(|e| format!("Failed to flush: {e}"))?;
    drop(local_file);

    // Update final stats
    if file_size == 0 {
        total.store(bytes_downloaded, Ordering::Relaxed);
    }
    completed.store(bytes_downloaded, Ordering::Relaxed);
    speed.store(0, Ordering::Relaxed);
    connections.store(0, Ordering::Relaxed);

    // Rename .part to final
    let final_path = super::ftp_download::finalize_download(&part_path, &filename, dir_path)?;
    tracing::info!("SFTP download complete: {}", final_path.display());
    Ok(final_path)
}

/// Try to authenticate via SSH key first, then password
async fn try_authenticate(
    session: &mut client::Handle<SshHandler>,
    user: &str,
    password: Option<&str>,
    private_key_source: Option<&str>,
    passphrase: Option<&str>,
) -> Result<bool, String> {
    // Try SSH key authentication
    if let Some(key_source) = private_key_source {
        if !key_source.is_empty() {
            match load_private_key(key_source, passphrase).await {
                Ok(key_pair) => {
                    match session
                        .authenticate_publickey(user, Arc::new(key_pair))
                        .await
                    {
                        Ok(true) => {
                            tracing::info!("SSH key authentication successful");
                            return Ok(true);
                        }
                        Ok(false) => {
                            tracing::warn!("SSH key authentication rejected by server");
                        }
                        Err(e) => {
                            tracing::warn!("SSH key authentication error: {e}");
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to load SSH key: {e}");
                }
            }
        }
    }

    // Fall back to password authentication
    if let Some(pass) = password {
        match session.authenticate_password(user, pass).await {
            Ok(true) => {
                tracing::info!("SSH password authentication successful");
                return Ok(true);
            }
            Ok(false) => {
                tracing::warn!("SSH password authentication rejected");
            }
            Err(e) => {
                tracing::warn!("SSH password authentication error: {e}");
            }
        }
    }

    Ok(false)
}

/// Load an SSH private key from either a file path or inline PEM content
async fn load_private_key(source: &str, passphrase: Option<&str>) -> Result<key::KeyPair, String> {
    let pem_content = if source.contains("-----BEGIN") {
        // Inline PEM content
        source.to_string()
    } else {
        // File path
        let path = if source.starts_with('~') {
            if let Some(home) = dirs::home_dir() {
                home.join(&source[2..])
            } else {
                PathBuf::from(source)
            }
        } else {
            PathBuf::from(source)
        };

        tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| format!("Failed to read SSH key file '{}': {e}", path.display()))?
    };

    russh_keys::decode_secret_key(&pem_content, passphrase)
        .map_err(|e| format!("Failed to decode SSH key: {e}"))
}

fn option_str(options: &Map<String, Value>, key: &str) -> Option<String> {
    options
        .get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}
