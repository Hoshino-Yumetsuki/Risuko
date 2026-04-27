//! Cloud upload sinks — push completed downloads to remote destinations

pub mod ftp;
pub mod manager;
pub mod rules;
pub mod s3;
pub mod sftp;
pub mod sink;
pub mod webdav;

pub use manager::{JobStatus, UploadJob, UploadSinkManager};
pub use rules::{RuleInput, RuleMatch, UploadRule};
pub use sink::{
    FtpConfig, PostUploadAction, S3Config, SftpConfig, SinkConfig, UploadControl, UploadFile,
    UploadProgress, UploadSink, UploadSinkRecord, WebdavConfig,
};

/// DTO produced by `TaskManager::files_for_upload` and consumed
/// by `UploadSinkManager::enqueue_for_file` — kept outside the trait to avoid
/// pulling task-internal types into the sink layer
#[derive(Debug, Clone)]
pub struct UploadFileSnapshot {
    pub local_path: std::path::PathBuf,
    pub remote_relative: String,
    pub size: u64,
}
