use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use futures_util::stream::{FuturesUnordered, StreamExt};
use russh::client;
use russh::keys::PrivateKeyWithHashAlg;
use russh_sftp::client::{error::Error as SftpError, RawSftpSession};
use russh_sftp::protocol::{FileAttributes, OpenFlags, StatusCode};
use serde_json::{Map, Value};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

use super::ftp_download::{http_proxy_from_options, option_str};
use super::FtpUri;
use crate::engine::speed_limiter::{SpeedEma, SpeedLimiter};
use crate::engine::ssh_known_hosts::TofuHandler;

const PART_SUFFIX: &str = ".part";
const BUF_SIZE: usize = 64 * 1024;
const SFTP_READ_AHEAD: usize = 6;

struct SftpReadChunk {
    offset: u64,
    requested_len: usize,
    data: Vec<u8>,
}

struct OrderedChunkBuffer {
    next_offset: u64,
    pending: BTreeMap<u64, Vec<u8>>,
}

impl OrderedChunkBuffer {
    fn new(next_offset: u64) -> Self {
        Self {
            next_offset,
            pending: BTreeMap::new(),
        }
    }

    fn push(&mut self, chunk: SftpReadChunk) {
        if !chunk.data.is_empty() {
            self.pending.insert(chunk.offset, chunk.data);
        }
    }

    fn pop_ready(&mut self) -> Option<Vec<u8>> {
        let data = self.pending.remove(&self.next_offset)?;
        self.next_offset += data.len() as u64;
        Some(data)
    }
}

fn next_sftp_read_len(file_size: u64, offset: u64) -> Option<usize> {
    if file_size > 0 {
        if offset >= file_size {
            None
        } else {
            Some(BUF_SIZE.min((file_size - offset) as usize))
        }
    } else {
        Some(BUF_SIZE)
    }
}

fn short_read_gap(chunk: &SftpReadChunk, file_size: u64) -> Option<(u64, usize)> {
    let actual_len = chunk.data.len();
    if actual_len == 0 || actual_len >= chunk.requested_len {
        return None;
    }

    let gap_offset = chunk.offset + actual_len as u64;
    if file_size > 0 && gap_offset >= file_size {
        return None;
    }

    let mut gap_len = chunk.requested_len - actual_len;
    if file_size > 0 {
        gap_len = gap_len.min((file_size - gap_offset) as usize);
    }
    (gap_len > 0).then_some((gap_offset, gap_len))
}

async fn read_sftp_range(
    sftp: Arc<RawSftpSession>,
    handle: String,
    offset: u64,
    len: usize,
) -> Result<SftpReadChunk, String> {
    let data = match sftp.read(handle, offset, len as u32).await {
        Ok(data) => data.data,
        Err(SftpError::Status(status)) if status.status_code == StatusCode::Eof => Vec::new(),
        Err(e) => return Err(format!("SFTP read error: {e}")),
    };

    Ok(SftpReadChunk {
        offset,
        requested_len: len,
        data,
    })
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
        super::ftp_download::basename_from_ftp_path(&parsed.path)
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
    let config = Arc::new(client::Config::default());

    let addr = format!("{}:{}", parsed.host, parsed.port);
    let http_proxy = http_proxy_from_options(options)?;
    let session_result = if let Some(proxy) = http_proxy {
        let stream = proxy
            .connect_tcp(&parsed.host, parsed.port)
            .await
            .map_err(|e| format!("SFTP proxy connect failed: {e}"))?;
        client::connect_stream(config, stream, TofuHandler::new(addr.clone())).await
    } else {
        client::connect(config, &addr, TofuHandler::new(addr.clone())).await
    }
    .map_err(|e| format!("SSH connect failed: {e}"));
    let mut session = session_result?;

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

    let mut sftp = RawSftpSession::new(channel.into_stream());
    let version = sftp
        .init()
        .await
        .map_err(|e| format!("SFTP session init failed: {e}"))?;
    if version
        .extensions
        .get(russh_sftp::extensions::LIMITS)
        .is_some_and(|v| v == "1")
    {
        match sftp.limits().await {
            Ok(limits) => sftp.set_limits(limits.into()),
            Err(e) => tracing::warn!("SFTP limits extension failed: {e}"),
        }
    }
    let sftp = Arc::new(sftp);

    connections.store(1, Ordering::Relaxed);

    // Stat remote file for size
    let remote_path = &parsed.path;
    let file_size = match sftp.stat(remote_path).await {
        Ok(attrs) => attrs.attrs.size.unwrap_or(0),
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

    let handle = sftp
        .open(remote_path, OpenFlags::READ, FileAttributes::empty())
        .await
        .map_err(|e| format!("SFTP open failed: {e}"))?
        .handle;

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

    // Use positional reads with a small read-ahead window for high-latency SFTP links
    let mut bytes_downloaded = resume_offset;
    let mut last_speed_time = Instant::now();
    let mut interval_bytes: u64 = 0;
    let mut ema = SpeedEma::new();
    let mut next_read_offset = resume_offset;
    let mut saw_eof = false;
    let mut reads = FuturesUnordered::new();
    let mut ordered = OrderedChunkBuffer::new(resume_offset);

    while reads.len() < SFTP_READ_AHEAD {
        let Some(len) = next_sftp_read_len(file_size, next_read_offset) else {
            break;
        };
        reads.push(read_sftp_range(
            sftp.clone(),
            handle.clone(),
            next_read_offset,
            len,
        ));
        next_read_offset += len as u64;
    }

    while !reads.is_empty() {
        if cancel_token.is_cancelled() {
            let _ = sftp.close(handle.clone()).await;
            return Err("Download cancelled".to_string());
        }

        let chunk = match tokio::select! {
            result = reads.next() => result,
            _ = cancel_token.cancelled() => {
                let _ = sftp.close(handle.clone()).await;
                return Err("Download cancelled".to_string());
            }
        } {
            Some(chunk) => chunk,
            None => break,
        };
        let chunk = chunk?;

        if chunk.data.is_empty() {
            saw_eof = true;
        } else {
            if let Some((gap_offset, gap_len)) = short_read_gap(&chunk, file_size) {
                reads.push(read_sftp_range(
                    sftp.clone(),
                    handle.clone(),
                    gap_offset,
                    gap_len,
                ));
            }
            ordered.push(chunk);

            while let Some(data) = ordered.pop_ready() {
                let n = data.len();
                global_limiter.acquire(n).await;
                task_limiter.acquire(n).await;

                local_file
                    .write_all(&data)
                    .await
                    .map_err(|e| format!("Failed to write: {e}"))?;

                bytes_downloaded += n as u64;
                completed.store(bytes_downloaded, Ordering::Relaxed);
                interval_bytes += n as u64;

                // Update speed EMA every 500 ms
                let elapsed = last_speed_time.elapsed();
                if elapsed.as_millis() >= 500 {
                    speed.store(
                        ema.update(interval_bytes, elapsed.as_secs_f64()),
                        Ordering::Relaxed,
                    );
                    interval_bytes = 0;
                    last_speed_time = Instant::now();
                }
            }
        }

        while !saw_eof && reads.len() < SFTP_READ_AHEAD {
            let Some(len) = next_sftp_read_len(file_size, next_read_offset) else {
                break;
            };
            reads.push(read_sftp_range(
                sftp.clone(),
                handle.clone(),
                next_read_offset,
                len,
            ));
            next_read_offset += len as u64;
        }
    }

    sftp.close(handle)
        .await
        .map_err(|e| format!("SFTP close failed: {e}"))?;

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
    let auto_rename = super::ftp_download::option_bool(options, "auto-file-renaming", true);
    let final_path =
        super::ftp_download::finalize_download(&part_path, &filename, dir_path, auto_rename)?;
    tracing::info!("SFTP download complete: {}", final_path.display());
    Ok(final_path)
}

/// Try to authenticate via SSH key first, then password
async fn try_authenticate(
    session: &mut client::Handle<TofuHandler>,
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
                    let key_with_alg = PrivateKeyWithHashAlg::new(Arc::new(key_pair), None);
                    match session.authenticate_publickey(user, key_with_alg).await {
                        Ok(auth) if auth.success() => {
                            tracing::info!("SSH key authentication successful");
                            return Ok(true);
                        }
                        Ok(_) => {
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
            Ok(auth) if auth.success() => {
                tracing::info!("SSH password authentication successful");
                return Ok(true);
            }
            Ok(_) => {
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
async fn load_private_key(
    source: &str,
    passphrase: Option<&str>,
) -> Result<russh::keys::PrivateKey, String> {
    let pem_content = if source.contains("----BEGIN") {
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

    russh::keys::decode_secret_key(&pem_content, passphrase)
        .map_err(|e| format!("Failed to decode SSH key: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(offset: u64, requested_len: usize, data: &[u8]) -> SftpReadChunk {
        SftpReadChunk {
            offset,
            requested_len,
            data: data.to_vec(),
        }
    }

    #[test]
    fn ordered_chunk_buffer_waits_for_missing_offsets() {
        let mut buffer = OrderedChunkBuffer::new(0);
        buffer.push(chunk(4, 4, b"efgh"));

        assert!(buffer.pop_ready().is_none());

        buffer.push(chunk(0, 4, b"abcd"));
        assert_eq!(buffer.pop_ready().as_deref(), Some(&b"abcd"[..]));
        assert_eq!(buffer.pop_ready().as_deref(), Some(&b"efgh"[..]));
        assert!(buffer.pop_ready().is_none());
    }

    #[test]
    fn short_read_gap_requests_only_the_missing_range() {
        let c = chunk(64, 16, b"abcd");

        assert_eq!(short_read_gap(&c, 100), Some((68, 12)));
        assert_eq!(short_read_gap(&c, 68), None);
        assert_eq!(short_read_gap(&chunk(64, 4, b"abcd"), 100), None);
    }

    #[test]
    fn next_sftp_read_len_caps_to_remaining_known_size() {
        assert_eq!(next_sftp_read_len(100, 0), Some(100));
        assert_eq!(next_sftp_read_len(100, 99), Some(1));
        assert_eq!(next_sftp_read_len(100, 100), None);
        assert_eq!(next_sftp_read_len(0, u64::MAX), Some(BUF_SIZE));
    }
}
