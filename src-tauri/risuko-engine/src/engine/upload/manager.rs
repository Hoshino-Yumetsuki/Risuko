//! `UploadSinkManager` — owns sinks/rules state, runs a bounded job queue,
//! emits lifecycle events. Mirrors the `RssManager` pattern so persistence
//! and event plumbing are the same shape

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::{watch, Mutex, RwLock, Semaphore};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::traits::{EventSink, StorageBackend};

use super::ftp::FtpSink;
use super::rules::{select_rule_sink, RuleInput, UploadRule};
use super::s3::S3Sink;
use super::sftp::SftpSink;
use super::sink::{
    BoxedSink, PostUploadAction, SinkConfig, UploadControl, UploadFile, UploadProgress,
    UploadSinkRecord,
};
use super::webdav::WebdavSink;

const UPLOAD_STORE_KEY: &str = "upload-sinks";

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[derive(Default, Serialize, Deserialize)]
struct UploadStore {
    #[serde(default)]
    sinks: Vec<UploadSinkRecord>,
    #[serde(default)]
    rules: Vec<UploadRule>,
    #[serde(default)]
    default_sink_id: Option<String>,
    #[serde(default = "default_concurrency")]
    max_concurrency: usize,
}

fn default_concurrency() -> usize {
    2
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum JobStatus {
    Queued,
    Active,
    Complete,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadJob {
    pub id: String,
    pub gid: String,
    pub sink_id: String,
    pub local_path: PathBuf,
    pub remote_relative: String,
    pub size: u64,
    pub uploaded: u64,
    pub status: JobStatus,
    pub error: Option<String>,
    pub created_at: u64,
    pub started_at: Option<u64>,
    pub finished_at: Option<u64>,
}

struct ActiveJob {
    progress: watch::Receiver<UploadProgress>,
}

pub struct UploadSinkManager {
    store: Arc<RwLock<UploadStore>>,
    storage: Arc<dyn StorageBackend>,
    event_sink: StdMutex<Arc<dyn EventSink>>,
    jobs: Arc<Mutex<HashMap<String, UploadJob>>>,
    active: Arc<Mutex<HashMap<String, ActiveJob>>>,
    /// Cancellation tokens for every spawned job — populated at enqueue
    /// time so even queued jobs (still waiting on the semaphore) can be
    /// cancelled by the user. Cleared once the job reaches a terminal state
    cancels: Arc<Mutex<HashMap<String, CancellationToken>>>,
    semaphore: Arc<RwLock<Arc<Semaphore>>>,
}

impl UploadSinkManager {
    pub fn new(storage: Arc<dyn StorageBackend>, event_sink: Arc<dyn EventSink>) -> Self {
        Self {
            store: Arc::new(RwLock::new(UploadStore {
                max_concurrency: default_concurrency(),
                ..Default::default()
            })),
            storage,
            event_sink: StdMutex::new(event_sink),
            jobs: Arc::new(Mutex::new(HashMap::new())),
            active: Arc::new(Mutex::new(HashMap::new())),
            cancels: Arc::new(Mutex::new(HashMap::new())),
            semaphore: Arc::new(RwLock::new(Arc::new(Semaphore::new(default_concurrency())))),
        }
    }

    /// Swap in a new event sink at runtime. Used by the Tauri host to upgrade
    /// from the bootstrap `NoopEventSink` to a `TauriEventSink` once the
    /// `AppHandle` is available
    pub fn set_event_sink(&self, sink: Arc<dyn EventSink>) {
        if let Ok(mut s) = self.event_sink.lock() {
            *s = sink;
        }
    }

    // persistence

    pub fn load(&self) -> Result<(), String> {
        if let Some(val) = self.storage.load(UPLOAD_STORE_KEY)? {
            if let Some(data_val) = val.get("data").cloned() {
                let data: UploadStore = serde_json::from_value(data_val)
                    .map_err(|e| format!("Failed to parse upload sinks: {e}"))?;
                let new_concurrency = data.max_concurrency.clamp(1, 16);
                let mut s = self.store.blocking_write();
                *s = data;
                s.max_concurrency = new_concurrency;
                drop(s);
                // Rebuild the semaphore with the persisted concurrency value
                // so configured limits actually apply at startup
                let mut sem = self.semaphore.blocking_write();
                *sem = Arc::new(Semaphore::new(new_concurrency));
            }
        }
        Ok(())
    }

    pub async fn save(&self) -> Result<(), String> {
        let data = {
            let s = self.store.read().await;
            serde_json::to_value(&*s).map_err(|e| format!("Serialize upload sinks failed: {e}"))?
        };
        let wrapper = serde_json::json!({ "data": data });
        self.storage.save(UPLOAD_STORE_KEY, &wrapper)?;
        Ok(())
    }

    // queries

    pub async fn list_sinks(&self) -> Vec<UploadSinkRecord> {
        self.store.read().await.sinks.clone()
    }

    pub async fn list_rules(&self) -> Vec<UploadRule> {
        self.store.read().await.rules.clone()
    }

    pub async fn default_sink_id(&self) -> Option<String> {
        self.store.read().await.default_sink_id.clone()
    }

    pub async fn list_jobs(&self) -> Vec<UploadJob> {
        let mut jobs: Vec<UploadJob> = self.jobs.lock().await.values().cloned().collect();
        // Stitch live progress in so the frontend sees fresh numbers without
        // a separate event per byte
        let active = self.active.lock().await;
        for job in jobs.iter_mut() {
            if let Some(a) = active.get(&job.id) {
                let p = a.progress.borrow();
                job.uploaded = p.uploaded;
            }
        }
        jobs.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        jobs
    }

    // sink CRUD

    pub async fn add_sink(&self, mut record: UploadSinkRecord) -> Result<UploadSinkRecord, String> {
        if record.id.is_empty() {
            record.id = Uuid::new_v4().to_string();
        }
        record.created_at = now_secs();
        // Sanity-check by trying to build the runtime — surfaces e.g. an
        // empty WebDAV endpoint at insert time, not at upload time
        let _ = build_sink_runtime(&record.config)?;

        let mut s = self.store.write().await;
        if s.sinks.iter().any(|x| x.id == record.id) {
            return Err(format!("sink id already exists: {}", record.id));
        }
        s.sinks.push(record.clone());
        if s.default_sink_id.is_none() {
            s.default_sink_id = Some(record.id.clone());
        }
        drop(s);
        self.save().await?;
        Ok(record)
    }

    pub async fn update_sink(&self, record: UploadSinkRecord) -> Result<(), String> {
        let _ = build_sink_runtime(&record.config)?;
        let mut s = self.store.write().await;
        let slot = s
            .sinks
            .iter_mut()
            .find(|x| x.id == record.id)
            .ok_or_else(|| format!("unknown sink {}", record.id))?;
        *slot = record;
        drop(s);
        self.save().await
    }

    pub async fn remove_sink(&self, id: &str) -> Result<(), String> {
        let mut s = self.store.write().await;
        let before = s.sinks.len();
        s.sinks.retain(|x| x.id != id);
        if s.sinks.len() == before {
            return Err(format!("unknown sink {id}"));
        }
        // Drop dangling references
        s.rules.retain(|r| r.sink_id != id);
        if s.default_sink_id.as_deref() == Some(id) {
            s.default_sink_id = s.sinks.first().map(|x| x.id.clone());
        }
        drop(s);
        self.save().await
    }

    pub async fn set_default_sink(&self, id: Option<String>) -> Result<(), String> {
        let mut s = self.store.write().await;
        if let Some(ref new_id) = id {
            if !s.sinks.iter().any(|x| &x.id == new_id) {
                return Err(format!("unknown sink {new_id}"));
            }
        }
        s.default_sink_id = id;
        drop(s);
        self.save().await
    }

    pub async fn set_max_concurrency(&self, n: usize) -> Result<(), String> {
        let n = n.clamp(1, 16);
        // Persist first; live resize follows. `tokio::Semaphore` cannot
        // shrink permits already handed out, but we can swap in a fresh
        // semaphore for future jobs — in-flight jobs keep their old permit
        let mut s = self.store.write().await;
        s.max_concurrency = n;
        drop(s);
        {
            let mut sem = self.semaphore.write().await;
            *sem = Arc::new(Semaphore::new(n));
        }
        self.save().await
    }

    // rule CRUD

    pub async fn add_rule(&self, mut rule: UploadRule) -> Result<UploadRule, String> {
        if rule.id.is_empty() {
            rule.id = Uuid::new_v4().to_string();
        }
        let mut s = self.store.write().await;
        if !s.sinks.iter().any(|x| x.id == rule.sink_id) {
            return Err(format!("rule references unknown sink {}", rule.sink_id));
        }
        s.rules.push(rule.clone());
        drop(s);
        self.save().await?;
        Ok(rule)
    }

    pub async fn remove_rule(&self, id: &str) -> Result<(), String> {
        let mut s = self.store.write().await;
        let before = s.rules.len();
        s.rules.retain(|r| r.id != id);
        if s.rules.len() == before {
            return Err(format!("unknown rule {id}"));
        }
        drop(s);
        self.save().await
    }

    pub async fn update_rule(&self, rule: UploadRule) -> Result<(), String> {
        let mut s = self.store.write().await;
        if !s.sinks.iter().any(|x| x.id == rule.sink_id) {
            return Err(format!("rule references unknown sink {}", rule.sink_id));
        }
        let slot = s
            .rules
            .iter_mut()
            .find(|r| r.id == rule.id)
            .ok_or_else(|| format!("unknown rule {}", rule.id))?;
        *slot = rule;
        drop(s);
        self.save().await
    }

    pub async fn test_sink(&self, id: &str) -> Result<(), String> {
        let cfg = {
            let s = self.store.read().await;
            s.sinks
                .iter()
                .find(|x| x.id == id)
                .map(|x| x.config.clone())
                .ok_or_else(|| format!("unknown sink {id}"))?
        };
        let runtime = build_sink_runtime(&cfg)?;
        runtime.test().await
    }

    // job control

    pub async fn cancel_job(&self, id: &str) -> Result<(), String> {
        // The cancel token is stored as soon as the job is enqueued so this
        // works for queued jobs (still waiting on the semaphore) too. The
        // spawned task checks the token after acquiring its permit and bails
        let cancels = self.cancels.lock().await;
        let token = cancels
            .get(id)
            .ok_or_else(|| format!("job {id} not found"))?
            .clone();
        drop(cancels);
        token.cancel();
        Ok(())
    }

    pub async fn clear_history(&self) {
        let mut jobs = self.jobs.lock().await;
        jobs.retain(|_, j| matches!(j.status, JobStatus::Queued | JobStatus::Active));
    }

    // enqueue (called from completion hook)

    /// Decide which sink, if any, should handle this file and enqueue if so
    /// `override_sink_id` is the per-task explicit choice (highest priority)
    pub async fn enqueue_for_file(
        self: &Arc<Self>,
        gid: &str,
        local_path: PathBuf,
        remote_relative: String,
        size: u64,
        category: Option<String>,
        task_kind: &str,
        override_sink_id: Option<String>,
    ) {
        let chosen_sink = self
            .pick_sink(
                &local_path,
                size,
                category.as_deref(),
                task_kind,
                override_sink_id,
            )
            .await;

        let Some(sink_id) = chosen_sink else {
            return;
        };

        // Snapshot sink record up front — we copy the config into the worker
        // so subsequent edits to the sink don't change an in-flight job
        let sink_record = {
            let s = self.store.read().await;
            match s.sinks.iter().find(|x| x.id == sink_id).cloned() {
                Some(r) => r,
                None => {
                    log::warn!("upload: sink {sink_id} disappeared before enqueue");
                    return;
                }
            }
        };

        let job_id = Uuid::new_v4().to_string();
        let job = UploadJob {
            id: job_id.clone(),
            gid: gid.to_string(),
            sink_id: sink_id.clone(),
            local_path: local_path.clone(),
            remote_relative: remote_relative.clone(),
            size,
            uploaded: 0,
            status: JobStatus::Queued,
            error: None,
            created_at: now_secs(),
            started_at: None,
            finished_at: None,
        };

        self.jobs.lock().await.insert(job_id.clone(), job.clone());
        // Pre-create the cancel token so users can cancel queued jobs that
        // haven't acquired a semaphore permit yet
        let cancel = CancellationToken::new();
        self.cancels
            .lock()
            .await
            .insert(job_id.clone(), cancel.clone());
        self.emit_event("engine:upload-queued", &job);

        let this = self.clone();
        tokio::spawn(async move {
            this.run_job(
                job_id,
                sink_record,
                local_path,
                remote_relative,
                size,
                cancel,
            )
            .await;
        });
    }

    async fn pick_sink(
        &self,
        local_path: &Path,
        size: u64,
        category: Option<&str>,
        task_kind: &str,
        override_sink_id: Option<String>,
    ) -> Option<String> {
        let s = self.store.read().await;

        // Per-task override wins, but must reference a real sink
        if let Some(id) = override_sink_id {
            if s.sinks.iter().any(|x| x.id == id) {
                return Some(id);
            }
        }

        // Rules in array order
        let input = RuleInput {
            file_path: local_path,
            size,
            category,
            task_kind,
        };
        if let Some(sink_id) = select_rule_sink(&s.rules, &input) {
            return Some(sink_id.to_string());
        }

        s.default_sink_id.clone()
    }

    async fn run_job(
        self: Arc<Self>,
        job_id: String,
        sink_record: UploadSinkRecord,
        local_path: PathBuf,
        remote_relative: String,
        size: u64,
        cancel: CancellationToken,
    ) {
        // Wait for a slot. Snapshot the current `Arc<Semaphore>` so a runtime
        // resize via `set_max_concurrency` doesn't deadlock us
        let sem = self.semaphore.read().await.clone();
        let permit = tokio::select! {
            res = sem.acquire_owned() => match res {
                Ok(p) => p,
                Err(_) => {
                    self.cancels.lock().await.remove(&job_id);
                    self.fail_job(&job_id, "engine shutdown").await;
                    return;
                }
            },
            _ = cancel.cancelled() => {
                // Cancelled while still queued — never acquired a permit
                self.cancels.lock().await.remove(&job_id);
                self.cancel_job_state(&job_id).await;
                return;
            }
        };

        // Cancellation may also have raced acquisition; bail before doing work
        if cancel.is_cancelled() {
            drop(permit);
            self.cancels.lock().await.remove(&job_id);
            self.cancel_job_state(&job_id).await;
            return;
        }

        let (tx, rx) = watch::channel(UploadProgress {
            uploaded: 0,
            total: size,
        });
        self.active
            .lock()
            .await
            .insert(job_id.clone(), ActiveJob { progress: rx });

        // Mark active
        if let Some(j) = self.jobs.lock().await.get_mut(&job_id) {
            j.status = JobStatus::Active;
            j.started_at = Some(now_secs());
            self.emit_event("engine:upload-start", j);
        }

        let sink_runtime = match build_sink_runtime(&sink_record.config) {
            Ok(r) => r,
            Err(e) => {
                drop(permit);
                self.cleanup_active(&job_id).await;
                self.fail_job(&job_id, &format!("sink init failed: {e}"))
                    .await;
                return;
            }
        };

        let file = UploadFile {
            local_path: local_path.clone(),
            remote_relative,
            size,
        };
        let ctl = UploadControl {
            cancel: cancel.clone(),
            progress: tx,
        };

        let result = sink_runtime.upload(&file, &ctl).await;
        drop(permit);
        self.cleanup_active(&job_id).await;

        match result {
            Ok(_remote_url) => {
                // Run post-upload action while we still have the path
                if let Err(e) = run_post_action(
                    &local_path,
                    sink_record.post_action,
                    sink_record.move_target.as_deref(),
                )
                .await
                {
                    log::warn!("upload {job_id}: post-action failed: {e}");
                }
                self.complete_job(&job_id).await;
                self.touch_sink_used(&sink_record.id).await;
            }
            Err(e) => {
                if cancel.is_cancelled() {
                    self.cancel_job_state(&job_id).await;
                } else {
                    self.fail_job(&job_id, &e).await;
                }
            }
        }
    }

    async fn cleanup_active(&self, job_id: &str) {
        self.active.lock().await.remove(job_id);
        self.cancels.lock().await.remove(job_id);
    }

    async fn complete_job(&self, job_id: &str) {
        let mut jobs = self.jobs.lock().await;
        if let Some(j) = jobs.get_mut(job_id) {
            j.status = JobStatus::Complete;
            j.uploaded = j.size;
            j.finished_at = Some(now_secs());
            let snapshot = j.clone();
            drop(jobs);
            self.emit_event("engine:upload-complete", &snapshot);
        }
    }

    async fn fail_job(&self, job_id: &str, error: &str) {
        let mut jobs = self.jobs.lock().await;
        if let Some(j) = jobs.get_mut(job_id) {
            j.status = JobStatus::Failed;
            j.error = Some(error.to_string());
            j.finished_at = Some(now_secs());
            let snapshot = j.clone();
            drop(jobs);
            self.emit_event("engine:upload-error", &snapshot);
        }
    }

    async fn cancel_job_state(&self, job_id: &str) {
        let mut jobs = self.jobs.lock().await;
        if let Some(j) = jobs.get_mut(job_id) {
            j.status = JobStatus::Cancelled;
            j.finished_at = Some(now_secs());
            let snapshot = j.clone();
            drop(jobs);
            self.emit_event("engine:upload-cancelled", &snapshot);
        }
    }

    async fn touch_sink_used(&self, sink_id: &str) {
        let mut s = self.store.write().await;
        if let Some(sink) = s.sinks.iter_mut().find(|x| x.id == sink_id) {
            sink.last_used_at = Some(now_secs());
        }
        drop(s);
        let _ = self.save().await;
    }

    fn emit_event(&self, name: &str, job: &UploadJob) {
        // Snapshot the current sink with a quick std-lock — virtually never
        // contended (only `set_event_sink` writes, once at startup)
        let sink = match self.event_sink.lock() {
            Ok(g) => g.clone(),
            Err(p) => p.into_inner().clone(),
        };
        match serde_json::to_value(job) {
            Ok(v) => sink.emit(name, v),
            Err(e) => log::warn!("upload event serialize failed: {e}"),
        }
    }
}

/// Build the runtime sink object from its persisted config. Centralised so
/// adding a new protocol means adding one match arm here plus one new module
pub fn build_sink_runtime(cfg: &SinkConfig) -> Result<BoxedSink, String> {
    match cfg {
        SinkConfig::Webdav(w) => {
            let s = WebdavSink::new(w.clone())?;
            Ok(Arc::new(s))
        }
        SinkConfig::S3(c) => {
            let s = S3Sink::new(c.clone())?;
            Ok(Arc::new(s))
        }
        SinkConfig::Sftp(c) => {
            let s = SftpSink::new(c.clone())?;
            Ok(Arc::new(s))
        }
        SinkConfig::Ftp(c) => {
            let s = FtpSink::new(c.clone())?;
            Ok(Arc::new(s))
        }
    }
}

async fn run_post_action(
    path: &Path,
    action: PostUploadAction,
    move_target: Option<&Path>,
) -> Result<(), String> {
    match action {
        PostUploadAction::Keep => Ok(()),
        PostUploadAction::Trash => {
            // Best-effort delete; trash crates aren't a workspace dep here.
            // Use unlink as a pragmatic fallback — the desktop app can swap
            // in its `trash_item` command via a future trait
            tokio::fs::remove_file(path)
                .await
                .map_err(|e| format!("remove {}: {e}", path.display()))
        }
        PostUploadAction::Move => {
            let target = match move_target {
                Some(t) => t,
                None => return Err("move post-action requires move_target".into()),
            };
            tokio::fs::create_dir_all(target)
                .await
                .map_err(|e| format!("mkdir {}: {e}", target.display()))?;
            let file_name = path
                .file_name()
                .ok_or_else(|| "source has no file name".to_string())?;
            let dst = target.join(file_name);
            // Try rename; if cross-device, fall back to copy + delete
            if tokio::fs::rename(path, &dst).await.is_err() {
                tokio::fs::copy(path, &dst)
                    .await
                    .map_err(|e| format!("copy {} -> {}: {e}", path.display(), dst.display()))?;
                tokio::fs::remove_file(path)
                    .await
                    .map_err(|e| format!("cleanup {}: {e}", path.display()))?;
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::upload::sink::{FtpConfig, S3Config, SftpConfig, SinkConfig, WebdavConfig};

    #[test]
    fn build_sink_runtime_webdav_ok() {
        let cfg = SinkConfig::Webdav(WebdavConfig {
            endpoint: "https://dav.example.com".into(),
            base_path: String::new(),
            username: String::new(),
            password: String::new(),
            insecure: false,
        });
        assert!(build_sink_runtime(&cfg).is_ok());
    }

    #[test]
    fn build_sink_runtime_webdav_invalid_endpoint() {
        let cfg = SinkConfig::Webdav(WebdavConfig {
            endpoint: String::new(),
            base_path: String::new(),
            username: String::new(),
            password: String::new(),
            insecure: false,
        });
        assert!(build_sink_runtime(&cfg).is_err());
    }

    #[test]
    fn build_sink_runtime_s3_ok() {
        let cfg = SinkConfig::S3(S3Config {
            endpoint: "https://s3.amazonaws.com".into(),
            region: "us-east-1".into(),
            bucket: "b".into(),
            access_key_id: "AKIA".into(),
            secret_access_key: "secret".into(),
            prefix: String::new(),
            force_path_style: false,
        });
        assert!(build_sink_runtime(&cfg).is_ok());
    }

    #[test]
    fn build_sink_runtime_s3_invalid_bucket() {
        let cfg = SinkConfig::S3(S3Config {
            endpoint: "https://s3.amazonaws.com".into(),
            region: "us-east-1".into(),
            bucket: String::new(),
            access_key_id: "AKIA".into(),
            secret_access_key: "secret".into(),
            prefix: String::new(),
            force_path_style: false,
        });
        assert!(build_sink_runtime(&cfg).is_err());
    }

    #[test]
    fn build_sink_runtime_sftp_ok() {
        let cfg = SinkConfig::Sftp(SftpConfig {
            host: "h".into(),
            port: 22,
            username: "u".into(),
            password: "p".into(),
            private_key: String::new(),
            base_path: String::new(),
        });
        assert!(build_sink_runtime(&cfg).is_ok());
    }

    #[test]
    fn build_sink_runtime_sftp_no_credentials() {
        let cfg = SinkConfig::Sftp(SftpConfig {
            host: "h".into(),
            port: 22,
            username: "u".into(),
            password: String::new(),
            private_key: String::new(),
            base_path: String::new(),
        });
        assert!(build_sink_runtime(&cfg).is_err());
    }

    #[test]
    fn build_sink_runtime_ftp_ok() {
        let cfg = SinkConfig::Ftp(FtpConfig {
            host: "ftp.example.com".into(),
            port: 21,
            username: String::new(),
            password: String::new(),
            base_path: String::new(),
            secure: false,
            insecure: false,
        });
        assert!(build_sink_runtime(&cfg).is_ok());
    }

    #[test]
    fn build_sink_runtime_ftp_empty_host() {
        let cfg = SinkConfig::Ftp(FtpConfig {
            host: String::new(),
            port: 21,
            username: String::new(),
            password: String::new(),
            base_path: String::new(),
            secure: false,
            insecure: false,
        });
        assert!(build_sink_runtime(&cfg).is_err());
    }
}
