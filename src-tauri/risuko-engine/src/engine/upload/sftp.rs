//! SFTP upload sink. Reuses the russh + russh-sftp stack already used by the SFTP downloader

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use russh::client;
use russh::keys::PrivateKeyWithHashAlg;
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::OpenFlags;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::sink::{SftpConfig, UploadControl, UploadFile, UploadSink};
use crate::engine::ssh_known_hosts::TofuHandler;

const COPY_BUF: usize = 64 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

pub struct SftpSink {
    cfg: SftpConfig,
}

impl SftpSink {
    pub fn new(cfg: SftpConfig) -> Result<Self, String> {
        if cfg.host.trim().is_empty() {
            return Err("SFTP host is empty".into());
        }
        if cfg.username.trim().is_empty() {
            return Err("SFTP username is empty".into());
        }
        if cfg.password.is_empty() && cfg.private_key.is_empty() {
            return Err("SFTP requires a password or a private key".into());
        }
        Ok(Self { cfg })
    }

    async fn connect(&self) -> Result<SftpSession, String> {
        let config = Arc::new(client::Config::default());
        let addr = format!("{}:{}", self.cfg.host, self.cfg.port);
        let handler = TofuHandler::new(addr.clone());
        let mut session = client::connect(config, &addr, handler)
            .await
            .map_err(|e| format!("SSH connect failed: {e}"))?;

        let mut authed = false;

        // Try private key first if provided
        if !self.cfg.private_key.is_empty() {
            match russh::keys::decode_secret_key(&self.cfg.private_key, None) {
                Ok(key) => {
                    let alg = PrivateKeyWithHashAlg::new(Arc::new(key), None);
                    match session
                        .authenticate_publickey(&self.cfg.username, alg)
                        .await
                    {
                        Ok(a) if a.success() => authed = true,
                        Ok(_) => log::warn!("SFTP key auth rejected"),
                        Err(e) => log::warn!("SFTP key auth error: {e}"),
                    }
                }
                Err(e) => log::warn!("SFTP key decode error: {e}"),
            }
        }

        if !authed && !self.cfg.password.is_empty() {
            match session
                .authenticate_password(&self.cfg.username, &self.cfg.password)
                .await
            {
                Ok(a) if a.success() => authed = true,
                Ok(_) => return Err("SFTP password auth rejected".into()),
                Err(e) => return Err(format!("SFTP password auth: {e}")),
            }
        }

        if !authed {
            return Err("SFTP authentication failed".into());
        }

        let channel = session
            .channel_open_session()
            .await
            .map_err(|e| format!("SSH channel open failed: {e}"))?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(|e| format!("SFTP subsystem request failed: {e}"))?;

        SftpSession::new(channel.into_stream())
            .await
            .map_err(|e| format!("SFTP session init failed: {e}"))
    }

    /// Walk the parent path one directory at a time creating any missing
    /// segments. SFTP `mkdir` errors when the parent doesn't exist
    async fn ensure_parent_dirs(&self, sftp: &SftpSession, full_path: &str) -> Result<(), String> {
        let parent = match full_path.rsplit_once('/') {
            Some((p, _)) if !p.is_empty() => p.to_string(),
            _ => return Ok(()),
        };

        let absolute = parent.starts_with('/');
        let mut accum = String::new();
        if absolute {
            accum.push('/');
        }
        for seg in parent.split('/').filter(|s| !s.is_empty()) {
            if !accum.is_empty() && !accum.ends_with('/') {
                accum.push('/');
            }
            accum.push_str(seg);
            // Best-effort: ignore "already exists" errors
            match sftp.try_exists(&accum).await {
                Ok(true) => continue,
                Ok(false) => {
                    if let Err(e) = sftp.create_dir(&accum).await {
                        // Tolerate races where another upload created it
                        log::debug!("SFTP mkdir {accum} ignored: {e}");
                    }
                }
                Err(_) => {
                    // Couldn't stat — try mkdir and ignore failure
                    let _ = sftp.create_dir(&accum).await;
                }
            }
        }
        Ok(())
    }

    fn full_remote_path(&self, remote_relative: &str) -> String {
        let base = self.cfg.base_path.trim_end_matches('/');
        let rel = remote_relative.trim_start_matches('/');
        if base.is_empty() {
            rel.to_string()
        } else {
            format!("{base}/{rel}")
        }
    }
}

#[async_trait]
impl UploadSink for SftpSink {
    async fn upload(&self, file: &UploadFile, ctl: &UploadControl) -> Result<String, String> {
        if ctl.cancel.is_cancelled() {
            return Err("cancelled".into());
        }

        let sftp = tokio::time::timeout(CONNECT_TIMEOUT, self.connect())
            .await
            .map_err(|_| "SFTP connect timed out".to_string())??;
        let remote = self.full_remote_path(&file.remote_relative);
        self.ensure_parent_dirs(&sftp, &remote).await?;

        let mut remote_file = sftp
            .open_with_flags(
                &remote,
                OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::TRUNCATE,
            )
            .await
            .map_err(|e| format!("SFTP open {remote}: {e}"))?;

        let local: PathBuf = file.local_path.clone();
        let mut local_file = tokio::fs::File::open(&local)
            .await
            .map_err(|e| format!("open {}: {e}", local.display()))?;

        let mut buf = vec![0u8; COPY_BUF];
        let mut sent: u64 = 0;

        // Helper: best-effort cleanup of a partially-written remote file.
        // Closes the handle (so the server releases its lock) and unlinks
        // the path so a retry doesn't see a torn file. Errors are logged
        // but never override the original failure
        async fn discard_partial(
            mut remote_file: russh_sftp::client::fs::File,
            sftp: &SftpSession,
            remote: &str,
        ) {
            let _ = remote_file.shutdown().await;
            if let Err(e) = sftp.remove_file(remote).await {
                log::debug!("SFTP cleanup of partial {remote} ignored: {e}");
            }
        }

        loop {
            if ctl.cancel.is_cancelled() {
                discard_partial(remote_file, &sftp, &remote).await;
                return Err("cancelled".into());
            }

            let n = match local_file.read(&mut buf).await {
                Ok(n) => n,
                Err(e) => {
                    discard_partial(remote_file, &sftp, &remote).await;
                    return Err(format!("local read: {e}"));
                }
            };
            if n == 0 {
                break;
            }
            if let Err(e) = remote_file.write_all(&buf[..n]).await {
                discard_partial(remote_file, &sftp, &remote).await;
                return Err(format!("SFTP write: {e}"));
            }
            sent += n as u64;
            ctl.report(sent, file.size.max(sent));
        }

        if let Err(e) = remote_file.shutdown().await {
            // Close failed after a complete write — still try to unlink so
            // a half-flushed file isn't left behind under our name
            if let Err(re) = sftp.remove_file(&remote).await {
                log::debug!("SFTP cleanup after close failure ignored: {re}");
            }
            return Err(format!("SFTP close: {e}"));
        }

        ctl.report(file.size, file.size);
        // Always insert exactly one '/' between port and the remote path so
        // the resulting URL is well-formed regardless of whether `remote`
        // is absolute or relative
        let remote_for_url = remote.trim_start_matches('/');
        Ok(format!(
            "sftp://{}@{}:{}/{}",
            self.cfg.username, self.cfg.host, self.cfg.port, remote_for_url
        ))
    }

    async fn test(&self) -> Result<(), String> {
        // Connect with a short overall timeout — if any step hangs we want
        // a fast failure in the UI
        tokio::time::timeout(Duration::from_secs(20), async {
            let sftp = self.connect().await?;
            // Stat the base dir or `/` to confirm we can talk SFTP
            let probe = if self.cfg.base_path.trim().is_empty() {
                "/".to_string()
            } else {
                self.cfg.base_path.clone()
            };
            sftp.try_exists(&probe)
                .await
                .map_err(|e| format!("stat {probe}: {e}"))?;
            Ok::<_, String>(())
        })
        .await
        .map_err(|_| "SFTP test timed out".to_string())?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(host: &str, user: &str, pass: &str, key: &str, base: &str) -> SftpConfig {
        SftpConfig {
            host: host.into(),
            port: 22,
            username: user.into(),
            password: pass.into(),
            private_key: key.into(),
            base_path: base.into(),
        }
    }

    #[test]
    fn rejects_empty_host() {
        assert!(SftpSink::new(cfg("", "u", "p", "", "")).is_err());
        assert!(SftpSink::new(cfg("   ", "u", "p", "", "")).is_err());
    }

    #[test]
    fn rejects_empty_username() {
        assert!(SftpSink::new(cfg("h", "", "p", "", "")).is_err());
    }

    #[test]
    fn rejects_no_credentials() {
        assert!(SftpSink::new(cfg("h", "u", "", "", "")).is_err());
    }

    #[test]
    fn accepts_password_only() {
        assert!(SftpSink::new(cfg("h", "u", "secret", "", "")).is_ok());
    }

    #[test]
    fn accepts_private_key_only() {
        assert!(SftpSink::new(cfg("h", "u", "", "PEM-DATA", "")).is_ok());
    }

    #[test]
    fn full_remote_no_base() {
        let s = SftpSink::new(cfg("h", "u", "p", "", "")).unwrap();
        assert_eq!(s.full_remote_path("foo/bar.bin"), "foo/bar.bin");
        assert_eq!(s.full_remote_path("/foo/bar.bin"), "foo/bar.bin");
    }

    #[test]
    fn full_remote_with_base() {
        let s = SftpSink::new(cfg("h", "u", "p", "", "/data/uploads")).unwrap();
        assert_eq!(
            s.full_remote_path("foo/bar.bin"),
            "/data/uploads/foo/bar.bin"
        );
    }

    #[test]
    fn full_remote_strips_trailing_base_slash() {
        let s = SftpSink::new(cfg("h", "u", "p", "", "/data/")).unwrap();
        assert_eq!(s.full_remote_path("file.bin"), "/data/file.bin");
    }

    #[test]
    fn full_remote_strips_leading_relative_slash() {
        let s = SftpSink::new(cfg("h", "u", "p", "", "/data")).unwrap();
        assert_eq!(s.full_remote_path("/file.bin"), "/data/file.bin");
    }
}
