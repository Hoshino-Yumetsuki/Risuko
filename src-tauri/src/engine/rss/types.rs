use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RssFeed {
    pub id: String,
    pub url: String,
    pub title: String,
    pub site_link: String,
    pub description: String,
    pub update_interval_secs: u64,
    pub last_fetched_at: Option<u64>,
    pub created_at: u64,
    pub is_active: bool,
    pub error_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RssItem {
    pub id: String,
    pub feed_id: String,
    pub title: String,
    pub link: String,
    pub pub_date: Option<u64>,
    pub description: String,
    pub enclosure_url: Option<String>,
    pub enclosure_type: Option<String>,
    pub enclosure_length: Option<u64>,
    pub is_read: bool,
    pub is_downloaded: bool,
    #[serde(default)]
    pub download_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RssRule {
    pub id: String,
    /// None means global (applies to all feeds)
    pub feed_id: Option<String>,
    pub name: String,
    pub pattern: String,
    pub is_regex: bool,
    pub is_active: bool,
    pub auto_download: bool,
    pub download_dir: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RssStore {
    pub feeds: Vec<RssFeed>,
    pub items: HashMap<String, Vec<RssItem>>,
    pub rules: Vec<RssRule>,
}

/// Maximum items kept per feed to prevent storage bloat
pub const MAX_ITEMS_PER_FEED: usize = 500;
/// Default update interval in seconds (30 minutes)
pub const DEFAULT_UPDATE_INTERVAL_SECS: u64 = 1800;
/// Auto-disable feed after this many consecutive failures
pub const MAX_CONSECUTIVE_ERRORS: u32 = 10;
