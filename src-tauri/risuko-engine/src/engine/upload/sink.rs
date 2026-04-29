//! `UploadSink` trait - protocol-specific upload backend interface
//!
//! Each protocol (WebDAV, S3, SFTP, FTP) implements this trait once. The
//! manager owns boxed instances and hands jobs to them. Adding a new protocol
//! is a matter of dropping in a new file under `engine/upload/<proto>.rs`,
//! implementing the trait, and wiring it in `manager::build_sink_runtime`

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

/// One-shot description of a file the manager wants the sink to deliver
#[derive(Debug, Clone)]
pub struct UploadFile {
    /// Absolute path to the source file on disk
    pub local_path: PathBuf,
    /// Path component to use under the sink's configured base — relative,
    /// uses `/` separators, never starts with `/`. The sink is responsible
    /// for creating any necessary parent directories
    pub remote_relative: String,
    /// Size in bytes
    pub size: u64,
}

/// Live progress reporter the sink writes to during upload
#[derive(Debug, Clone)]
pub struct UploadProgress {
    pub uploaded: u64,
    pub total: u64,
}

/// Cancellation + progress channel passed to the sink
#[derive(Clone)]
pub struct UploadControl {
    pub cancel: CancellationToken,
    pub progress: watch::Sender<UploadProgress>,
}

impl UploadControl {
    pub fn report(&self, uploaded: u64, total: u64) {
        let _ = self.progress.send(UploadProgress { uploaded, total });
    }
}

/// Per-sink protocol-specific configuration. Stored verbatim in user config
///
/// Discriminated by `kind` for serde compatibility with the TS-side type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum SinkConfig {
    Webdav(WebdavConfig),
    S3(S3Config),
    Sftp(SftpConfig),
    Ftp(FtpConfig),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WebdavConfig {
    /// Base endpoint URL — e.g. `https://example.com/remote.php/dav/files/me`
    pub endpoint: String,
    /// Optional sub-folder under the endpoint
    #[serde(default)]
    pub base_path: String,
    /// Optional basic-auth username
    #[serde(default)]
    pub username: String,
    /// Optional basic-auth password. Not persisted to disk — the frontend
    /// must re-supply on engine restart
    #[serde(default, skip_serializing)]
    pub password: String,
    /// Skip TLS verification (self-signed homelab servers)
    #[serde(default)]
    pub insecure: bool,
}

/// S3-compatible object storage. Works with AWS S3, MinIO, Backblaze B2 (S3
/// API), Cloudflare R2, Wasabi, Garage, etc. Uses SigV4 single-PUT
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct S3Config {
    /// Endpoint URL — e.g. `https://s3.amazonaws.com` or
    /// `http://minio.local:9000`. Trailing slash is tolerated
    pub endpoint: String,
    /// Region — used for SigV4 signing scope
    #[serde(default)]
    pub region: String,
    /// Bucket name
    pub bucket: String,
    /// Access key ID
    pub access_key_id: String,
    /// Secret access key. Not persisted to disk — the frontend must
    /// re-supply on engine restart
    #[serde(default, skip_serializing)]
    pub secret_access_key: String,
    /// Optional key prefix prepended to every object key
    #[serde(default)]
    pub prefix: String,
    /// Use bucket-in-path URLs (`/{bucket}/{key}`) instead of virtual-host
    /// style (`{bucket}.host/{key}`). Required for MinIO and most self-hosted
    #[serde(default)]
    pub force_path_style: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SftpConfig {
    pub host: String,
    #[serde(default = "default_sftp_port")]
    pub port: u16,
    pub username: String,
    /// Password auth. Used if non-empty; otherwise tries the private key.
    /// Not persisted to disk — the frontend must re-supply on engine restart
    #[serde(default, skip_serializing)]
    pub password: String,
    /// PEM-encoded OpenSSH or PKCS#1 private key. Not persisted to disk
    #[serde(default, skip_serializing)]
    pub private_key: String,
    /// Remote base directory (defaults to login home if empty)
    #[serde(default)]
    pub base_path: String,
}

fn default_sftp_port() -> u16 {
    22
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FtpConfig {
    pub host: String,
    #[serde(default = "default_ftp_port")]
    pub port: u16,
    #[serde(default)]
    pub username: String,
    /// Not persisted to disk — the frontend must re-supply on engine restart
    #[serde(default, skip_serializing)]
    pub password: String,
    /// Remote base directory (created on demand)
    #[serde(default)]
    pub base_path: String,
    /// Use explicit FTPS (AUTH TLS) on the control channel
    #[serde(default)]
    pub secure: bool,
    /// Skip TLS verification (self-signed homelab servers)
    #[serde(default)]
    pub insecure: bool,
}

fn default_ftp_port() -> u16 {
    21
}

/// What the manager should do with the local file once the sink reports success
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PostUploadAction {
    Keep,
    Trash,
    /// Move into a configured directory (path resolved at action time)
    Move,
}

impl Default for PostUploadAction {
    fn default() -> Self {
        Self::Keep
    }
}

/// Persisted sink record — what the user added in Preferences. Holds the
/// runtime config plus identity / display metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadSinkRecord {
    pub id: String,
    pub label: String,
    pub config: SinkConfig,
    #[serde(default)]
    pub post_action: PostUploadAction,
    /// Optional move-target directory when post_action == Move
    #[serde(default)]
    pub move_target: Option<PathBuf>,
    pub created_at: u64,
    #[serde(default)]
    pub last_used_at: Option<u64>,
}

/// Per-sink protocol implementation. Implementations should be cheap to
/// construct (configs are passed by value) — the manager builds an instance
/// for the duration of a single job
#[async_trait]
pub trait UploadSink: Send + Sync {
    /// Upload `file` to the sink's destination, reporting progress and
    /// honouring cancellation. Returns the canonical remote URL on success
    /// (best-effort — empty string is acceptable when the sink can't construct
    /// one, e.g. SFTP without a public URL)
    async fn upload(&self, file: &UploadFile, ctl: &UploadControl) -> Result<String, String>;

    /// Test connectivity / authentication. Should be a quick round-trip
    async fn test(&self) -> Result<(), String>;
}

/// Helper alias: a fresh boxed sink built from a `SinkConfig`
pub type BoxedSink = Arc<dyn UploadSink>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webdav_config_round_trip_camel_case() {
        let cfg = SinkConfig::Webdav(WebdavConfig {
            endpoint: "https://dav.example.com".into(),
            base_path: "uploads".into(),
            username: "u".into(),
            // password is skip_serializing; use empty so round-trip equality holds
            password: String::new(),
            insecure: true,
        });
        let json = serde_json::to_string(&cfg).unwrap();
        // Discriminant + camelCase fields
        assert!(json.contains("\"kind\":\"webdav\""));
        assert!(json.contains("\"basePath\":\"uploads\""));
        let back: SinkConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
    }

    #[test]
    fn s3_config_serializes_camel_case() {
        let cfg = SinkConfig::S3(S3Config {
            endpoint: "https://s3.amazonaws.com".into(),
            region: "us-west-2".into(),
            bucket: "mybucket".into(),
            access_key_id: "AKIA".into(),
            // secret_access_key is skip_serializing; use empty for round-trip equality
            secret_access_key: String::new(),
            prefix: "folder".into(),
            force_path_style: true,
        });
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(json.contains("\"kind\":\"s3\""));
        assert!(json.contains("\"accessKeyId\":\"AKIA\""));
        assert!(json.contains("\"forcePathStyle\":true"));
        let back: SinkConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
    }

    #[test]
    fn sftp_default_port_22() {
        // Omit `port` from the JSON — should default to 22
        let json = r#"{"kind":"sftp","host":"h","username":"u"}"#;
        let cfg: SinkConfig = serde_json::from_str(json).unwrap();
        match cfg {
            SinkConfig::Sftp(c) => {
                assert_eq!(c.port, 22);
                assert_eq!(c.password, "");
                assert_eq!(c.private_key, "");
                assert_eq!(c.base_path, "");
            }
            _ => panic!("expected sftp"),
        }
    }

    #[test]
    fn ftp_default_port_21() {
        let json = r#"{"kind":"ftp","host":"h"}"#;
        let cfg: SinkConfig = serde_json::from_str(json).unwrap();
        match cfg {
            SinkConfig::Ftp(c) => {
                assert_eq!(c.port, 21);
                assert!(!c.secure);
                assert_eq!(c.username, "");
            }
            _ => panic!("expected ftp"),
        }
    }

    #[test]
    fn post_action_serde_kebab_case() {
        assert_eq!(
            serde_json::to_string(&PostUploadAction::Keep).unwrap(),
            "\"keep\""
        );
        assert_eq!(
            serde_json::to_string(&PostUploadAction::Trash).unwrap(),
            "\"trash\""
        );
        assert_eq!(
            serde_json::to_string(&PostUploadAction::Move).unwrap(),
            "\"move\""
        );
        assert_eq!(PostUploadAction::default(), PostUploadAction::Keep);
    }

    #[test]
    fn upload_sink_record_round_trip() {
        let rec = UploadSinkRecord {
            id: "id-1".into(),
            label: "My Sink".into(),
            config: SinkConfig::Webdav(WebdavConfig {
                endpoint: "https://e".into(),
                base_path: String::new(),
                username: String::new(),
                password: String::new(),
                insecure: false,
            }),
            post_action: PostUploadAction::Trash,
            move_target: None,
            created_at: 1_700_000_000,
            last_used_at: Some(1_700_000_100),
        };
        let json = serde_json::to_string(&rec).unwrap();
        assert!(json.contains("\"postAction\":\"trash\""));
        assert!(json.contains("\"createdAt\":1700000000"));
        let back: UploadSinkRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, rec.id);
        assert_eq!(back.post_action, rec.post_action);
    }

    #[tokio::test]
    async fn upload_control_report_propagates() {
        let (tx, rx) = watch::channel(UploadProgress {
            uploaded: 0,
            total: 0,
        });
        let ctl = UploadControl {
            cancel: CancellationToken::new(),
            progress: tx,
        };
        ctl.report(50, 100);
        let snap = rx.borrow().clone();
        assert_eq!(snap.uploaded, 50);
        assert_eq!(snap.total, 100);
    }
}
