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

    /// Returned together so callers keep the russh `Handle` alive for the
    /// lifetime of the SFTP session. Dropping the `Handle` issues an SSH
    /// `Disconnect`, which tears down the channel that `SftpSession` is
    /// streaming over and turns the next SFTP request into an error
    /// (observed as `russh::client: drop handle` immediately after the
    /// SFTP `VERSION` reply, followed by an SFTP `STATUS` failure on the
    /// first real request)
    async fn connect(&self) -> Result<(client::Handle<TofuHandler>, SftpSession), String> {
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

        let sftp = SftpSession::new(channel.into_stream())
            .await
            .map_err(|e| format!("SFTP session init failed: {e}"))?;
        Ok((session, sftp))
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
        // A configured base of "/" is an absolute root mount and must be
        // preserved — naive trim_end_matches('/') would collapse it to "" and
        // produce a non-absolute path
        let rel = remote_relative.trim_start_matches('/');
        if self.cfg.base_path == "/" {
            return format!("/{rel}");
        }
        let base = self.cfg.base_path.trim_end_matches('/');
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

        let (_ssh, sftp) = tokio::time::timeout(CONNECT_TIMEOUT, self.connect())
            .await
            .map_err(|_| "SFTP connect timed out".to_string())?
            .inspect_err(|e| log::error!("SFTP connect failed: {e}"))?;
        let remote = self.full_remote_path(&file.remote_relative);
        log::debug!("SFTP upload starting: remote={remote}");
        self.ensure_parent_dirs(&sftp, &remote)
            .await
            .inspect_err(|e| log::debug!("SFTP ensure_parent_dirs({remote}) failed: {e}"))?;

        // Stage writes to a sibling `.part` file so an existing good upload
        // at the final path is never truncated or unlinked on failure.
        // Only the successful end-state (full body flushed + closed) renames
        // the temp into the final name
        let remote_tmp = format!("{remote}.part");
        let mut remote_file = sftp
            .open_with_flags(
                &remote_tmp,
                OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::TRUNCATE,
            )
            .await
            .map_err(|e| {
                // Don't echo `remote_tmp` (user-controlled, may include
                // home directory or other host layout details) into
                // the returned error or info logs. The full path is
                // still emitted at debug level for operators with
                // log access
                log::debug!("SFTP open {remote_tmp}: {e}");
                log::error!("SFTP open failed: {e}");
                format!("SFTP open failed: {e}")
            })?;

        let local: PathBuf = file.local_path.clone();
        let mut local_file = tokio::fs::File::open(&local)
            .await
            .map_err(|e| format!("open {}: {e}", local.display()))?;

        let mut buf = vec![0u8; COPY_BUF];
        let mut sent: u64 = 0;

        // Helper: best-effort cleanup of the partially-written temp file.
        // Closes the handle (so the server releases its lock) and unlinks
        // the temp path so a retry doesn't see a torn file. Errors are
        // logged but never override the original failure. Crucially this
        // never touches the final `remote` name
        async fn discard_partial(
            mut remote_file: russh_sftp::client::fs::File,
            sftp: &SftpSession,
            remote_tmp: &str,
        ) {
            let _ = remote_file.shutdown().await;
            if let Err(e) = sftp.remove_file(remote_tmp).await {
                log::debug!("SFTP cleanup of partial {remote_tmp} ignored: {e}");
            }
        }

        loop {
            if ctl.cancel.is_cancelled() {
                discard_partial(remote_file, &sftp, &remote_tmp).await;
                return Err("cancelled".into());
            }

            let n = match local_file.read(&mut buf).await {
                Ok(n) => n,
                Err(e) => {
                    discard_partial(remote_file, &sftp, &remote_tmp).await;
                    return Err(format!("local read: {e}"));
                }
            };
            if n == 0 {
                break;
            }
            if let Err(e) = remote_file.write_all(&buf[..n]).await {
                discard_partial(remote_file, &sftp, &remote_tmp).await;
                return Err(format!("SFTP write: {e}"));
            }
            sent += n as u64;
            ctl.report(sent, file.size.max(sent));
        }

        if let Err(e) = remote_file.shutdown().await {
            // Close failed after a complete write — drop the temp so a
            // half-flushed file isn't left behind. Final `remote` is
            // untouched
            if let Err(re) = sftp.remove_file(&remote_tmp).await {
                log::debug!("SFTP cleanup after close failure ignored: {re}");
            }
            return Err(format!("SFTP close: {e}"));
        }

        // Rename the staged temp over the final path. SFTPv3 `rename` may
        // fail if the destination already exists on some servers; on that
        // failure unlink the existing remote and retry once so users can
        // overwrite previous uploads
        if let Err(e) = sftp.rename(&remote_tmp, &remote).await {
            log::debug!("SFTP rename {remote_tmp} -> {remote} failed ({e}); retrying after unlink");
            let _ = sftp.remove_file(&remote).await;
            if let Err(e2) = sftp.rename(&remote_tmp, &remote).await {
                let _ = sftp.remove_file(&remote_tmp).await;
                return Err(format!("SFTP rename {remote_tmp} -> {remote}: {e2}"));
            }
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
            let (_ssh, sftp) = self.connect().await?;
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
