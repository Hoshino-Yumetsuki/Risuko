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
    /// Resolved file-category key (matches `RuleMatch::categories` values:
    /// "music", "video", "image", "document", "compressed", "program")
    /// `None` when the extension doesn't fall into any known bucket
    pub category: Option<String>,
}

/// Resolve a file's category bucket from its name/extension. Mirrors the
/// extension table used by the Tauri-layer `resolve_file_category` command
/// so rule matching here agrees with the user-facing category-dirs feature
pub fn resolve_category(name: &str) -> Option<String> {
    if name.is_empty() {
        return None;
    }
    let dot = name.rfind('.')?;
    if dot == 0 || dot >= name.len() - 1 {
        return None;
    }
    let ext = name[dot..].to_ascii_lowercase();
    static MUSIC: &[&str] = &[
        ".aac", ".ape", ".flac", ".flav", ".m4a", ".mp3", ".ogg", ".wav", ".wma",
    ];
    static VIDEO: &[&str] = &[
        ".avi", ".m3u8", ".m4v", ".mkv", ".mov", ".mp4", ".mpg", ".rmvb", ".ts", ".vob", ".wmv",
    ];
    static IMAGE: &[&str] = &[
        ".ai", ".bmp", ".eps", ".fig", ".gif", ".heic", ".icn", ".ico", ".jpeg", ".jpg", ".png",
        ".psd", ".raw", ".sketch", ".svg", ".tif", ".webp", ".xd",
    ];
    static DOCUMENT: &[&str] = &[
        ".azw3", ".csv", ".doc", ".docx", ".epub", ".key", ".mobi", ".numbers", ".pages", ".pdf",
        ".ppt", ".pptx", ".txt", ".xsl", ".xslx",
    ];
    static COMPRESSED: &[&str] = &[
        ".zip", ".rar", ".7z", ".tar", ".gz", ".bz2", ".xz", ".zst", ".iso",
    ];
    static PROGRAM: &[&str] = &[
        ".exe",
        ".msi",
        ".dmg",
        ".pkg",
        ".deb",
        ".rpm",
        ".appimage",
        ".apk",
    ];
    for &(cat, table) in &[
        ("music", MUSIC),
        ("video", VIDEO),
        ("image", IMAGE),
        ("document", DOCUMENT),
        ("compressed", COMPRESSED),
        ("program", PROGRAM),
    ] {
        if table.contains(&ext.as_str()) {
            return Some(cat.to_string());
        }
    }
    None
}
