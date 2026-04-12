pub mod types;

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use regex::Regex;
use serde_json::Value;
use tauri::{AppHandle, Emitter};
use tauri_plugin_store::StoreExt;
use tokio::sync::Mutex;
use uuid::Uuid;

use self::types::*;

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
    app: AppHandle,
}

impl RssManager {
    pub fn new(app: &AppHandle) -> Self {
        Self {
            store: Arc::new(Mutex::new(RssStore::default())),
            app: app.clone(),
        }
    }

    // Persistence

    pub fn load(&self) -> Result<(), String> {
        let store = self
            .app
            .store(RSS_STORE_KEY)
            .map_err(|e| format!("Failed to open RSS store: {e}"))?;
        if let Some(val) = store.get("data") {
            let data: RssStore = serde_json::from_value(val)
                .map_err(|e| format!("Failed to parse RSS data: {e}"))?;
            let mut s = self.store.blocking_lock();
            *s = data;
        }
        Ok(())
    }

    pub async fn save(&self) -> Result<(), String> {
        let data = {
            let s = self.store.lock().await;
            serde_json::to_value(&*s).map_err(|e| format!("Serialize RSS data failed: {e}"))?
        };
        let store = self
            .app
            .store(RSS_STORE_KEY)
            .map_err(|e| format!("Failed to open RSS store: {e}"))?;
        store.set("data", data);
        store
            .save()
            .map_err(|e| format!("Failed to save RSS store: {e}"))?;
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
                merged.extend(existing.drain(..));
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

    pub fn start_polling(rss: Arc<Self>) -> tauri::async_runtime::JoinHandle<()> {
        tauri::async_runtime::spawn(async move {
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
                    let _ = rss.app.emit("rss-new-items", total_new);
                }
            }
        })
    }
}

// Helpers

async fn fetch_feed_bytes(url: &str) -> Result<Vec<u8>, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?;

    let resp = client
        .get(url)
        .header("User-Agent", "Motrix/1.0")
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
                    content.size.map(|s| s as u64),
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
                link.length.map(|l| l as u64),
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
