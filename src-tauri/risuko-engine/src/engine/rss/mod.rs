pub mod types;

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use regex::Regex;
use serde_json::Value;
use tokio::sync::Mutex;
use uuid::Uuid;

use self::types::*;
use crate::traits::{EventSink, StorageBackend};

const RSS_STORE_KEY: &str = "rss";

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn item_id(guid_or_link: &str) -> String {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(guid_or_link.as_bytes());
    hash.iter().map(|b| format!("{b:02x}")).collect()
}

pub struct RssManager {
    store: Arc<Mutex<RssStore>>,
    storage: Arc<dyn StorageBackend>,
    event_sink: Arc<dyn EventSink>,
}

impl RssManager {
    pub fn new(storage: Arc<dyn StorageBackend>, event_sink: Arc<dyn EventSink>) -> Self {
        Self {
            store: Arc::new(Mutex::new(RssStore::default())),
            storage,
            event_sink,
        }
    }

    // Persistence

    pub fn load(&self) -> Result<(), String> {
        if let Some(val) = self.storage.load(RSS_STORE_KEY)? {
            if let Some(data_val) = val.get("data").cloned() {
                let data: RssStore = serde_json::from_value(data_val)
                    .map_err(|e| format!("Failed to parse RSS data: {e}"))?;
                let mut s = self.store.blocking_lock();
                *s = data;
            }
        }
        Ok(())
    }

    pub async fn save(&self) -> Result<(), String> {
        let data = {
            let s = self.store.lock().await;
            serde_json::to_value(&*s).map_err(|e| format!("Serialize RSS data failed: {e}"))?
        };
        let wrapper = serde_json::json!({ "data": data });
        self.storage.save(RSS_STORE_KEY, &wrapper)?;
        Ok(())
    }

    // Feed CRUD

    pub async fn add_feed(&self, url: &str) -> Result<RssFeed, String> {
        let body = fetch_feed_bytes(url).await?;
        let parsed =
            feed_rs::parser::parse(&body[..]).map_err(|e| format!("Failed to parse feed: {e}"))?;

        let title = parsed
            .title
            .map(|t| t.content)
            .unwrap_or_else(|| url.to_string());
        let site_link = parsed
            .links
            .first()
            .map(|l| l.href.clone())
            .unwrap_or_default();
        let description = parsed.description.map(|d| d.content).unwrap_or_default();

        let feed = RssFeed {
            id: Uuid::new_v4().to_string(),
            url: url.to_string(),
            title,
            site_link,
            description,
            update_interval_secs: DEFAULT_UPDATE_INTERVAL_SECS,
            last_fetched_at: Some(now_secs()),
            created_at: now_secs(),
            is_active: true,
            error_count: 0,
        };

        let items = extract_items(&feed.id, &parsed.entries);

        {
            let mut s = self.store.lock().await;
            // Prevent duplicate URL
            if s.feeds.iter().any(|f| f.url == url) {
                return Err("Feed already subscribed".to_string());
            }
            s.feeds.push(feed.clone());
            let mut items_list = items;
            items_list.truncate(MAX_ITEMS_PER_FEED);
            s.items.insert(feed.id.clone(), items_list);
        }

        self.save().await?;
        Ok(feed)
    }

    pub async fn remove_feed(&self, feed_id: &str) -> Result<(), String> {
        let mut s = self.store.lock().await;
        s.feeds.retain(|f| f.id != feed_id);
        s.items.remove(feed_id);
        s.rules.retain(|r| r.feed_id.as_deref() != Some(feed_id));
        drop(s);
        self.save().await
    }

    pub async fn update_feed(&self, feed_id: &str) -> Result<Vec<RssItem>, String> {
        let url = {
            let s = self.store.lock().await;
            s.feeds
                .iter()
                .find(|f| f.id == feed_id)
                .map(|f| f.url.clone())
                .ok_or_else(|| "Feed not found".to_string())?
        };

        let result = fetch_and_parse(&url).await;

        let mut s = self.store.lock().await;
        let feed = s
            .feeds
            .iter_mut()
            .find(|f| f.id == feed_id)
            .ok_or_else(|| "Feed not found".to_string())?;

        match result {
            Ok(parsed) => {
                feed.last_fetched_at = Some(now_secs());
                feed.error_count = 0;
                if let Some(title) = parsed.title {
                    feed.title = title.content;
                }

                let new_items = extract_items(feed_id, &parsed.entries);
                let existing = s.items.entry(feed_id.to_string()).or_default();
                let existing_ids: std::collections::HashSet<&str> =
                    existing.iter().map(|i| i.id.as_str()).collect();

                let mut fresh: Vec<RssItem> = Vec::new();
                for item in new_items {
                    if !existing_ids.contains(item.id.as_str()) {
                        fresh.push(item);
                    }
                }

                // Prepend new items
                let mut merged = fresh.clone();
                merged.append(existing);
                merged.truncate(MAX_ITEMS_PER_FEED);
                *existing = merged;

                drop(s);
                self.save().await?;
                Ok(fresh)
            }
            Err(e) => {
                feed.error_count += 1;
                if feed.error_count >= MAX_CONSECUTIVE_ERRORS {
                    log::warn!(
                        "Feed '{}' disabled after {} consecutive errors",
                        feed.title,
                        feed.error_count
                    );
                    feed.is_active = false;
                }
                drop(s);
                self.save().await?;
                Err(e)
            }
        }
    }

    pub async fn update_all_feeds(&self) -> Vec<(String, Vec<RssItem>)> {
        let feeds: Vec<(String, bool)> = {
            let s = self.store.lock().await;
            s.feeds
                .iter()
                .map(|f| (f.id.clone(), f.is_active))
                .collect()
        };

        let mut all_new = Vec::new();
        for (feed_id, is_active) in feeds {
            if !is_active {
                continue;
            }
            match self.update_feed(&feed_id).await {
                Ok(new_items) if !new_items.is_empty() => {
                    all_new.push((feed_id, new_items));
                }
                Ok(_) => {}
                Err(e) => {
                    log::warn!("Failed to update feed {}: {}", feed_id, e);
                }
            }
        }
        all_new
    }

    pub async fn get_feeds(&self) -> Vec<RssFeed> {
        self.store.lock().await.feeds.clone()
    }

    pub async fn get_items(&self, feed_id: &str) -> Vec<RssItem> {
        self.store
            .lock()
            .await
            .items
            .get(feed_id)
            .cloned()
            .unwrap_or_default()
    }

    pub async fn update_feed_settings(
        &self,
        feed_id: &str,
        interval: Option<u64>,
        is_active: Option<bool>,
    ) -> Result<(), String> {
        let mut s = self.store.lock().await;
        let feed = s
            .feeds
            .iter_mut()
            .find(|f| f.id == feed_id)
            .ok_or_else(|| "Feed not found".to_string())?;
        if let Some(interval) = interval {
            feed.update_interval_secs = interval;
        }
        if let Some(active) = is_active {
            feed.is_active = active;
            if active {
                feed.error_count = 0;
            }
        }
        drop(s);
        self.save().await
    }

    // Item operations

    pub async fn mark_item_downloaded(
        &self,
        feed_id: &str,
        item_id: &str,
        download_path: Option<String>,
    ) -> Result<(), String> {
        let mut s = self.store.lock().await;
        if let Some(items) = s.items.get_mut(feed_id) {
            if let Some(item) = items.iter_mut().find(|i| i.id == item_id) {
                item.is_downloaded = true;
                item.is_read = true;
                item.download_path = download_path;
            }
        }
        drop(s);
        self.save().await
    }

    pub async fn clear_item_download(&self, feed_id: &str, item_id: &str) -> Result<(), String> {
        let mut s = self.store.lock().await;
        let mut path_to_delete: Option<String> = None;
        if let Some(items) = s.items.get_mut(feed_id) {
            if let Some(item) = items.iter_mut().find(|i| i.id == item_id) {
                path_to_delete = item.download_path.take();
                item.is_downloaded = false;
            }
        }
        drop(s);

        if let Some(path) = path_to_delete {
            let p = std::path::Path::new(&path);
            if p.exists() {
                if let Err(e) = tokio::fs::remove_file(p).await {
                    log::warn!("Failed to delete downloaded file {}: {}", path, e);
                }
            }
        }

        self.save().await
    }

    pub async fn delete_items(
        &self,
        items_by_feed: Vec<(String, Vec<String>)>,
    ) -> Result<(), String> {
        let mut s = self.store.lock().await;
        let mut paths_to_delete: Vec<String> = Vec::new();
        for (feed_id, item_ids) in &items_by_feed {
            if let Some(items) = s.items.get_mut(feed_id) {
                let id_set: std::collections::HashSet<&str> =
                    item_ids.iter().map(|s| s.as_str()).collect();
                // Collect download paths before removing
                for item in items.iter() {
                    if id_set.contains(item.id.as_str()) {
                        if let Some(ref path) = item.download_path {
                            paths_to_delete.push(path.clone());
                        }
                    }
                }
                items.retain(|i| !id_set.contains(i.id.as_str()));
            }
        }
        drop(s);

        // Delete downloaded files (best-effort)
        for path in &paths_to_delete {
            let p = std::path::Path::new(path);
            if p.exists() {
                if let Err(e) = tokio::fs::remove_file(p).await {
                    log::warn!("Failed to delete downloaded file {}: {}", path, e);
                }
            }
        }

        self.save().await
    }

    pub async fn get_item_download_url(
        &self,
        feed_id: &str,
        item_id: &str,
    ) -> Result<String, String> {
        let s = self.store.lock().await;
        let items = s
            .items
            .get(feed_id)
            .ok_or_else(|| "Feed not found".to_string())?;
        let item = items
            .iter()
            .find(|i| i.id == item_id)
            .ok_or_else(|| "Item not found".to_string())?;

        item.enclosure_url
            .clone()
            .or_else(|| {
                if !item.link.is_empty() {
                    Some(item.link.clone())
                } else {
                    None
                }
            })
            .ok_or_else(|| "No downloadable URL found for this item".to_string())
    }

    pub async fn get_item_download_path(
        &self,
        feed_id: &str,
        item_id: &str,
    ) -> Result<String, String> {
        let s = self.store.lock().await;
        let items = s
            .items
            .get(feed_id)
            .ok_or_else(|| "Feed not found".to_string())?;
        let item = items
            .iter()
            .find(|i| i.id == item_id)
            .ok_or_else(|| "Item not found".to_string())?;

        item.download_path
            .clone()
            .ok_or_else(|| "No download path recorded".to_string())
    }

    // Rules

    pub async fn add_rule(&self, rule: RssRule) -> Result<RssRule, String> {
        // Validate regex if applicable
        if rule.is_regex {
            Regex::new(&rule.pattern).map_err(|e| format!("Invalid regex pattern: {e}"))?;
        }
        let rule = RssRule {
            id: Uuid::new_v4().to_string(),
            ..rule
        };
        let mut s = self.store.lock().await;
        s.rules.push(rule.clone());
        drop(s);
        self.save().await?;
        Ok(rule)
    }

    pub async fn remove_rule(&self, rule_id: &str) -> Result<(), String> {
        let mut s = self.store.lock().await;
        s.rules.retain(|r| r.id != rule_id);
        drop(s);
        self.save().await
    }

    pub async fn get_rules(&self) -> Vec<RssRule> {
        self.store.lock().await.rules.clone()
    }

    /// Returns the first matching active rule for the given item, or None
    pub async fn match_rules(&self, item: &RssItem) -> Option<RssRule> {
        let s = self.store.lock().await;
        for rule in &s.rules {
            if !rule.is_active || !rule.auto_download {
                continue;
            }
            // Rule must apply globally or to this item's feed
            if let Some(ref rule_feed_id) = rule.feed_id {
                if rule_feed_id != &item.feed_id {
                    continue;
                }
            }
            if matches_pattern(&rule.pattern, rule.is_regex, &item.title) {
                return Some(rule.clone());
            }
        }
        None
    }

    // Polling

    pub fn start_polling(rss: Arc<Self>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            // Initial delay before first poll
            tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;

            loop {
                let min_interval = {
                    let s = rss.store.lock().await;
                    s.feeds
                        .iter()
                        .filter(|f| f.is_active)
                        .map(|f| f.update_interval_secs)
                        .min()
                        .unwrap_or(DEFAULT_UPDATE_INTERVAL_SECS)
                };

                tokio::time::sleep(tokio::time::Duration::from_secs(min_interval)).await;

                let new_items_per_feed = rss.update_all_feeds().await;

                // Auto-download matching items
                for (feed_id, new_items) in &new_items_per_feed {
                    for item in new_items {
                        if item.is_downloaded {
                            continue;
                        }
                        if let Some(rule) = rss.match_rules(item).await {
                            if let Ok(url) =
                                rss.get_item_download_url(&item.feed_id, &item.id).await
                            {
                                let options = rule.download_dir.as_ref().map(|dir| {
                                    let mut map = serde_json::Map::new();
                                    map.insert("dir".to_string(), Value::String(dir.clone()));
                                    Value::Object(map)
                                });

                                // Use the engine add_uri to start download
                                if let Some(manager) = super::get_manager().await {
                                    let opts = match options {
                                        Some(Value::Object(map)) => map,
                                        _ => serde_json::Map::new(),
                                    };
                                    if let Err(e) =
                                        manager.add_http_task(vec![url.clone()], opts).await
                                    {
                                        log::warn!(
                                            "Auto-download failed for '{}': {}",
                                            item.title,
                                            e
                                        );
                                    } else {
                                        let _ =
                                            rss.mark_item_downloaded(feed_id, &item.id, None).await;
                                        log::info!(
                                            "Auto-downloaded '{}' via rule '{}'",
                                            item.title,
                                            rule.name
                                        );
                                    }
                                }
                            }
                        }
                    }
                }

                // Notify frontend of new items
                if !new_items_per_feed.is_empty() {
                    let total_new: usize = new_items_per_feed
                        .iter()
                        .map(|(_, items)| items.len())
                        .sum();
                    rss.event_sink
                        .emit("rss-new-items", serde_json::json!(total_new));
                }
            }
        })
    }
}

// Helpers

/// Shared HTTP client for RSS feed fetches. Building a fresh `Client` on every
/// call would rebuild TLS state and drop keep-alive between polls, so cache
/// one for the lifetime of the process. Returns an error rather than panicking
/// if the underlying TLS/connector setup fails so a transient init failure
/// becomes a recoverable RSS fetch error
fn http_client() -> Result<&'static risuko_http::Client, String> {
    static CLIENT: std::sync::OnceLock<risuko_http::Client> = std::sync::OnceLock::new();
    if let Some(c) = CLIENT.get() {
        return Ok(c);
    }
    let client = risuko_http::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("Risuko/1.0")
        .build()
        .map_err(|e| format!("Failed to build rss http client: {e}"))?;
    // If another thread won the race, our `client` is dropped and we return
    // the one already stored
    let _ = CLIENT.set(client);
    Ok(CLIENT.get().expect("client just initialized"))
}

async fn fetch_feed_bytes(url: &str) -> Result<Vec<u8>, String> {
    let resp = http_client()?
        .get(url)
        .send()
        .await
        .map_err(|e| format!("Failed to fetch feed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("Feed returned HTTP {}", resp.status()));
    }

    resp.bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| format!("Failed to read feed body: {e}"))
}

async fn fetch_and_parse(url: &str) -> Result<feed_rs::model::Feed, String> {
    let body = fetch_feed_bytes(url).await?;
    feed_rs::parser::parse(&body[..]).map_err(|e| format!("Failed to parse feed: {e}"))
}

fn extract_items(feed_id: &str, entries: &[feed_rs::model::Entry]) -> Vec<RssItem> {
    entries
        .iter()
        .map(|entry| {
            let guid = entry.id.clone();
            let link = entry
                .links
                .first()
                .map(|l| l.href.clone())
                .unwrap_or_default();

            let id_source = if guid.is_empty() { &link } else { &guid };
            let id = item_id(id_source);

            let title = entry
                .title
                .as_ref()
                .map(|t| t.content.clone())
                .unwrap_or_default();

            let description = entry
                .summary
                .as_ref()
                .map(|s| s.content.clone())
                .or_else(|| entry.content.as_ref().and_then(|c| c.body.clone()))
                .unwrap_or_default();

            let pub_date = entry
                .published
                .or(entry.updated)
                .map(|dt| dt.timestamp() as u64);

            // Extract enclosure: prefer media content, then links with enclosure type
            let (enc_url, enc_type, enc_len) = extract_enclosure(entry);

            RssItem {
                id,
                feed_id: feed_id.to_string(),
                title,
                link,
                pub_date,
                description,
                enclosure_url: enc_url,
                enclosure_type: enc_type,
                enclosure_length: enc_len,
                is_read: false,
                is_downloaded: false,
                download_path: None,
            }
        })
        .collect()
}

fn extract_enclosure(
    entry: &feed_rs::model::Entry,
) -> (Option<String>, Option<String>, Option<u64>) {
    // Try media objects first
    for media in &entry.media {
        for content in &media.content {
            if let Some(ref url) = content.url {
                return (
                    Some(url.to_string()),
                    content.content_type.as_ref().map(|m| m.to_string()),
                    content.size,
                );
            }
        }
    }

    // Then try links with rel="enclosure"
    for link in &entry.links {
        if link.rel.as_deref() == Some("enclosure") {
            return (
                Some(link.href.clone()),
                link.media_type.clone(),
                link.length,
            );
        }
    }

    (None, None, None)
}

fn matches_pattern(pattern: &str, is_regex: bool, text: &str) -> bool {
    if is_regex {
        Regex::new(pattern)
            .map(|re| re.is_match(text))
            .unwrap_or(false)
    } else {
        let lower_text = text.to_lowercase();
        let lower_pattern = pattern.to_lowercase();
        lower_text.contains(&lower_pattern)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traits::{FileStorage, NoopEventSink};
    use tempfile::TempDir;

    struct RssTestCtx {
        _dir: TempDir,
        mgr: RssManager,
    }

    fn test_manager() -> RssTestCtx {
        let dir = TempDir::new().unwrap();
        let storage: Arc<dyn StorageBackend> = Arc::new(FileStorage::new(dir.path().to_path_buf()));
        let event_sink: Arc<dyn EventSink> = Arc::new(NoopEventSink);
        let mgr = RssManager::new(storage, event_sink);
        RssTestCtx { _dir: dir, mgr }
    }

    fn sample_feed(id: &str, url: &str) -> RssFeed {
        RssFeed {
            id: id.into(),
            url: url.into(),
            title: "Test Feed".into(),
            site_link: "https://example.com".into(),
            description: "desc".into(),
            update_interval_secs: DEFAULT_UPDATE_INTERVAL_SECS,
            last_fetched_at: None,
            created_at: 1,
            is_active: true,
            error_count: 0,
        }
    }

    fn sample_item(feed_id: &str, item_id: &str, title: &str) -> RssItem {
        RssItem {
            id: item_id.into(),
            feed_id: feed_id.into(),
            title: title.into(),
            link: "https://example.com/item".into(),
            pub_date: Some(12345),
            description: "desc".into(),
            enclosure_url: Some("https://example.com/file.torrent".into()),
            enclosure_type: None,
            enclosure_length: None,
            is_read: false,
            is_downloaded: false,
            download_path: None,
        }
    }

    // -- Pure helpers --

    #[test]
    fn item_id_is_deterministic() {
        let a = item_id("hello");
        let b = item_id("hello");
        let c = item_id("world");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 64); // SHA-256 hex
    }

    #[test]
    fn matches_pattern_substring() {
        assert!(matches_pattern("hello", false, "Hello World"));
        assert!(!matches_pattern("xyz", false, "Hello World"));
    }

    #[test]
    fn matches_pattern_regex() {
        assert!(matches_pattern(r"\d+", true, "Episode 42"));
        assert!(!matches_pattern(r"^\d+$", true, "Episode 42"));
    }

    #[test]
    fn matches_pattern_invalid_regex_returns_false() {
        assert!(!matches_pattern(r"[invalid", true, "text"));
    }

    // -- RssManager CRUD --

    #[test]
    fn get_feeds_returns_populated() {
        let ctx = test_manager();
        let mgr = &ctx.mgr;
        let rt = tokio::runtime::Runtime::new().unwrap();
        {
            let mut s = mgr.store.blocking_lock();
            s.feeds.push(sample_feed("f1", "https://a.com"));
        }
        let feeds = rt.block_on(mgr.get_feeds());
        assert_eq!(feeds.len(), 1);
        assert_eq!(feeds[0].id, "f1");
    }

    #[test]
    fn get_items_returns_feed_items() {
        let ctx = test_manager();
        let mgr = &ctx.mgr;
        let rt = tokio::runtime::Runtime::new().unwrap();
        {
            let mut s = mgr.store.blocking_lock();
            s.feeds.push(sample_feed("f1", "https://a.com"));
            s.items
                .insert("f1".into(), vec![sample_item("f1", "i1", "Item 1")]);
        }
        let items = rt.block_on(mgr.get_items("f1"));
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Item 1");
    }

    #[test]
    fn get_items_missing_feed_returns_empty() {
        let ctx = test_manager();
        let mgr = &ctx.mgr;
        let rt = tokio::runtime::Runtime::new().unwrap();
        let items = rt.block_on(mgr.get_items("none"));
        assert!(items.is_empty());
    }

    #[test]
    fn remove_feed_deletes_items_and_rules() {
        let ctx = test_manager();
        let mgr = &ctx.mgr;
        let rt = tokio::runtime::Runtime::new().unwrap();
        {
            let mut s = mgr.store.blocking_lock();
            s.feeds.push(sample_feed("f1", "https://a.com"));
            s.items
                .insert("f1".into(), vec![sample_item("f1", "i1", "Item 1")]);
            s.rules.push(RssRule {
                id: "r1".into(),
                feed_id: Some("f1".into()),
                name: "Rule".into(),
                pattern: "*".into(),
                is_regex: false,
                is_active: true,
                auto_download: true,
                download_dir: None,
            });
        }
        rt.block_on(mgr.remove_feed("f1")).unwrap();
        let feeds = rt.block_on(mgr.get_feeds());
        assert!(feeds.is_empty());
        let items = rt.block_on(mgr.get_items("f1"));
        assert!(items.is_empty());
        let rules = rt.block_on(mgr.get_rules());
        assert!(rules.is_empty());
    }

    #[test]
    fn update_feed_settings_changes_interval_and_active() {
        let ctx = test_manager();
        let mgr = &ctx.mgr;
        let rt = tokio::runtime::Runtime::new().unwrap();
        {
            let mut s = mgr.store.blocking_lock();
            s.feeds.push(sample_feed("f1", "https://a.com"));
        }
        rt.block_on(mgr.update_feed_settings("f1", Some(60), Some(false)))
            .unwrap();
        let feeds = rt.block_on(mgr.get_feeds());
        assert_eq!(feeds[0].update_interval_secs, 60);
        assert!(!feeds[0].is_active);
    }

    #[test]
    fn update_feed_settings_not_found() {
        let ctx = test_manager();
        let mgr = &ctx.mgr;
        let rt = tokio::runtime::Runtime::new().unwrap();
        let err = rt
            .block_on(mgr.update_feed_settings("none", Some(60), None))
            .unwrap_err();
        assert!(err.contains("not found"));
    }

    #[test]
    fn mark_item_downloaded_updates_state() {
        let ctx = test_manager();
        let mgr = &ctx.mgr;
        let rt = tokio::runtime::Runtime::new().unwrap();
        {
            let mut s = mgr.store.blocking_lock();
            s.feeds.push(sample_feed("f1", "https://a.com"));
            s.items
                .insert("f1".into(), vec![sample_item("f1", "i1", "Item 1")]);
        }
        rt.block_on(mgr.mark_item_downloaded("f1", "i1", Some("/downloads/file.txt".into())))
            .unwrap();
        let items = rt.block_on(mgr.get_items("f1"));
        assert!(items[0].is_downloaded);
        assert!(items[0].is_read);
        assert_eq!(items[0].download_path, Some("/downloads/file.txt".into()));
    }

    #[test]
    fn get_item_download_url_prefers_enclosure() {
        let ctx = test_manager();
        let mgr = &ctx.mgr;
        let rt = tokio::runtime::Runtime::new().unwrap();
        {
            let mut s = mgr.store.blocking_lock();
            s.feeds.push(sample_feed("f1", "https://a.com"));
            let mut item = sample_item("f1", "i1", "Item 1");
            item.enclosure_url = Some("https://enc.example.com/file.torrent".into());
            item.link = "https://link.example.com/".into();
            s.items.insert("f1".into(), vec![item]);
        }
        let url = rt.block_on(mgr.get_item_download_url("f1", "i1")).unwrap();
        assert_eq!(url, "https://enc.example.com/file.torrent");
    }

    #[test]
    fn get_item_download_url_falls_back_to_link() {
        let ctx = test_manager();
        let mgr = &ctx.mgr;
        let rt = tokio::runtime::Runtime::new().unwrap();
        {
            let mut s = mgr.store.blocking_lock();
            s.feeds.push(sample_feed("f1", "https://a.com"));
            let mut item = sample_item("f1", "i1", "Item 1");
            item.enclosure_url = None;
            item.link = "https://link.example.com/".into();
            s.items.insert("f1".into(), vec![item]);
        }
        let url = rt.block_on(mgr.get_item_download_url("f1", "i1")).unwrap();
        assert_eq!(url, "https://link.example.com/");
    }

    #[test]
    fn get_item_download_url_no_url() {
        let ctx = test_manager();
        let mgr = &ctx.mgr;
        let rt = tokio::runtime::Runtime::new().unwrap();
        {
            let mut s = mgr.store.blocking_lock();
            s.feeds.push(sample_feed("f1", "https://a.com"));
            let mut item = sample_item("f1", "i1", "Item 1");
            item.enclosure_url = None;
            item.link = String::new();
            s.items.insert("f1".into(), vec![item]);
        }
        let err = rt
            .block_on(mgr.get_item_download_url("f1", "i1"))
            .unwrap_err();
        assert!(err.contains("No downloadable URL"));
    }

    #[test]
    fn add_rule_generates_id_and_validates_regex() {
        let ctx = test_manager();
        let mgr = &ctx.mgr;
        let rt = tokio::runtime::Runtime::new().unwrap();
        let rule = RssRule {
            id: String::new(),
            feed_id: None,
            name: "Global".into(),
            pattern: r".*".into(),
            is_regex: true,
            is_active: true,
            auto_download: false,
            download_dir: None,
        };
        let created = rt.block_on(mgr.add_rule(rule)).unwrap();
        assert!(!created.id.is_empty());
        let rules = rt.block_on(mgr.get_rules());
        assert_eq!(rules.len(), 1);
    }

    #[test]
    fn add_rule_rejects_invalid_regex() {
        let ctx = test_manager();
        let mgr = &ctx.mgr;
        let rt = tokio::runtime::Runtime::new().unwrap();
        let rule = RssRule {
            id: String::new(),
            feed_id: None,
            name: "Bad".into(),
            pattern: r"[bad".into(),
            is_regex: true,
            is_active: true,
            auto_download: false,
            download_dir: None,
        };
        let err = rt.block_on(mgr.add_rule(rule)).unwrap_err();
        assert!(err.contains("Invalid regex"));
    }

    #[test]
    fn remove_rule_deletes() {
        let ctx = test_manager();
        let mgr = &ctx.mgr;
        let rt = tokio::runtime::Runtime::new().unwrap();
        let rule = RssRule {
            id: String::new(),
            feed_id: None,
            name: "Rule".into(),
            pattern: "test".into(),
            is_regex: false,
            is_active: true,
            auto_download: true,
            download_dir: None,
        };
        let created = rt.block_on(mgr.add_rule(rule)).unwrap();
        rt.block_on(mgr.remove_rule(&created.id)).unwrap();
        let rules = rt.block_on(mgr.get_rules());
        assert!(rules.is_empty());
    }

    #[test]
    fn match_rules_substring() {
        let ctx = test_manager();
        let mgr = &ctx.mgr;
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(mgr.add_rule(RssRule {
            id: "r1".into(),
            feed_id: Some("f1".into()),
            name: "Match".into(),
            pattern: "hello".into(),
            is_regex: false,
            is_active: true,
            auto_download: true,
            download_dir: None,
        }))
        .unwrap();
        let item = RssItem {
            id: "i1".into(),
            feed_id: "f1".into(),
            title: "Hello World".into(),
            link: String::new(),
            pub_date: None,
            description: String::new(),
            enclosure_url: None,
            enclosure_type: None,
            enclosure_length: None,
            is_read: false,
            is_downloaded: false,
            download_path: None,
        };
        let matched = rt.block_on(mgr.match_rules(&item));
        assert!(matched.is_some());
    }

    #[test]
    fn match_rules_respects_feed_scope() {
        let ctx = test_manager();
        let mgr = &ctx.mgr;
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(mgr.add_rule(RssRule {
            id: "r1".into(),
            feed_id: Some("f1".into()),
            name: "Scope".into(),
            pattern: "hello".into(),
            is_regex: false,
            is_active: true,
            auto_download: true,
            download_dir: None,
        }))
        .unwrap();
        let item = RssItem {
            id: "i1".into(),
            feed_id: "f2".into(),
            title: "Hello World".into(),
            link: String::new(),
            pub_date: None,
            description: String::new(),
            enclosure_url: None,
            enclosure_type: None,
            enclosure_length: None,
            is_read: false,
            is_downloaded: false,
            download_path: None,
        };
        let matched = rt.block_on(mgr.match_rules(&item));
        assert!(matched.is_none());
    }

    #[test]
    fn match_rules_skips_inactive() {
        let ctx = test_manager();
        let mgr = &ctx.mgr;
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(mgr.add_rule(RssRule {
            id: "r1".into(),
            feed_id: None,
            name: "Inactive".into(),
            pattern: "hello".into(),
            is_regex: false,
            is_active: false,
            auto_download: true,
            download_dir: None,
        }))
        .unwrap();
        let item = RssItem {
            id: "i1".into(),
            feed_id: "f1".into(),
            title: "Hello World".into(),
            link: String::new(),
            pub_date: None,
            description: String::new(),
            enclosure_url: None,
            enclosure_type: None,
            enclosure_length: None,
            is_read: false,
            is_downloaded: false,
            download_path: None,
        };
        let matched = rt.block_on(mgr.match_rules(&item));
        assert!(matched.is_none());
    }

    #[test]
    fn delete_items_removes_selected() {
        let ctx = test_manager();
        let mgr = &ctx.mgr;
        let rt = tokio::runtime::Runtime::new().unwrap();
        {
            let mut s = mgr.store.blocking_lock();
            s.feeds.push(sample_feed("f1", "https://a.com"));
            s.items.insert(
                "f1".into(),
                vec![
                    sample_item("f1", "i1", "A"),
                    sample_item("f1", "i2", "B"),
                    sample_item("f1", "i3", "C"),
                ],
            );
        }
        rt.block_on(mgr.delete_items(vec![("f1".into(), vec!["i1".into(), "i3".into()])]))
            .unwrap();
        let items = rt.block_on(mgr.get_items("f1"));
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "i2");
    }

    #[test]
    fn load_and_save_round_trip() {
        let dir = TempDir::new().unwrap();
        let storage: Arc<dyn StorageBackend> = Arc::new(FileStorage::new(dir.path().to_path_buf()));
        let event_sink: Arc<dyn EventSink> = Arc::new(NoopEventSink);
        let mgr = RssManager::new(storage.clone(), event_sink.clone());
        let rt = tokio::runtime::Runtime::new().unwrap();
        {
            let mut s = mgr.store.blocking_lock();
            s.feeds.push(sample_feed("f1", "https://a.com"));
            s.items
                .insert("f1".into(), vec![sample_item("f1", "i1", "Item")]);
        }
        rt.block_on(mgr.save()).unwrap();

        let mgr2 = RssManager::new(storage, event_sink);
        mgr2.load().unwrap();
        let feeds = rt.block_on(mgr2.get_feeds());
        assert_eq!(feeds.len(), 1);
        let items = rt.block_on(mgr2.get_items("f1"));
        assert_eq!(items.len(), 1);
    }
}
