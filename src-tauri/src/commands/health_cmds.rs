//! Aggregated health checks surfaced by the `/health` panel
//!
//! Exposes a single `run_health_checks` Tauri command that runs cheap probes
//! against engine + system state and returns a structured report. Slow probes
//! (e.g. proxy reachability) are gated behind `slow_probes`. Live BT internals
//! (DHT node count, UPnP mappings, LSD peers) are reported as config-only flags
//! for now — exposing them properly requires new accessors in `risuko-bt`

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::{Duration, SystemTime};

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, State};

#[cfg(not(target_os = "android"))]
use tauri_plugin_autostart::ManagerExt;

use risuko_engine::engine::{self, media, options::EngineOptions, torrent::BtHealthSnapshot};

use crate::commands::event_cmds::sleep_inhibit_active;
use crate::state::AppState;

const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const LOG_TAIL_BYTES: u64 = 1_048_576; // 1 MiB
const PROXY_PROBE_URL: &str = "https://www.cloudflare.com/cdn-cgi/trace";
const PROXY_PROBE_TIMEOUT: Duration = Duration::from_secs(4);

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum HealthStatus {
    Ok,
    Warn,
    Fail,
    Skipped,
}

impl HealthStatus {
    fn rank(self) -> u8 {
        match self {
            Self::Ok => 0,
            Self::Skipped => 1,
            Self::Warn => 2,
            Self::Fail => 3,
        }
    }
    fn worst(self, other: Self) -> Self {
        if self.rank() >= other.rank() {
            self
        } else {
            other
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthFix {
    pub kind: &'static str,
    pub target: Option<String>,
}

impl HealthFix {
    fn open_pref(target: &'static str) -> Self {
        Self {
            kind: "open-preference",
            target: Some(target.to_string()),
        }
    }
    fn restart_engine() -> Self {
        Self {
            kind: "restart-engine",
            target: None,
        }
    }
    fn open_log_dir() -> Self {
        Self {
            kind: "open-log-dir",
            target: None,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthCheck {
    pub id: String,
    pub status: HealthStatus,
    pub message: String,
    pub hint: Option<String>,
    pub fix: Option<HealthFix>,
    pub details: Option<Value>,
}

impl HealthCheck {
    fn ok(id: &str, msg: impl Into<String>) -> Self {
        Self::new(id, HealthStatus::Ok, msg, None, None)
    }
    fn warn(id: &str, msg: impl Into<String>, fix: Option<HealthFix>) -> Self {
        Self::new(id, HealthStatus::Warn, msg, None, fix)
    }
    fn fail(id: &str, msg: impl Into<String>, fix: Option<HealthFix>) -> Self {
        Self::new(id, HealthStatus::Fail, msg, None, fix)
    }
    fn skipped(id: &str, msg: impl Into<String>) -> Self {
        Self::new(id, HealthStatus::Skipped, msg, None, None)
    }
    fn new(
        id: &str,
        status: HealthStatus,
        msg: impl Into<String>,
        hint: Option<String>,
        fix: Option<HealthFix>,
    ) -> Self {
        Self {
            id: id.to_string(),
            status,
            message: msg.into(),
            hint,
            fix,
            details: None,
        }
    }
    fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthCategory {
    pub id: &'static str,
    pub status: HealthStatus,
    pub checks: Vec<HealthCheck>,
}

impl HealthCategory {
    fn from_checks(id: &'static str, checks: Vec<HealthCheck>) -> Self {
        let status = checks
            .iter()
            .map(|c| c.status)
            .fold(HealthStatus::Ok, HealthStatus::worst);
        Self { id, status, checks }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthReport {
    pub generated_at_ms: u64,
    pub engine_running: bool,
    pub overall_status: HealthStatus,
    pub categories: Vec<HealthCategory>,
    pub log_path: String,
}

#[tauri::command]
pub async fn run_health_checks(
    handle: AppHandle,
    state: State<'_, AppState>,
    categories: Option<Vec<String>>,
    slow_probes: Option<bool>,
) -> Result<HealthReport, String> {
    let slow = slow_probes.unwrap_or(false);
    // Snapshot config + engine state up front so we don't hold locks across awaits
    let (system_cfg, user_cfg, log_dir) = {
        let cfg = state.config.lock().map_err(|e| e.to_string())?;
        (
            cfg.get_system_config().clone(),
            cfg.get_user_config().clone(),
            state.log_dir.clone(),
        )
    };
    let options = EngineOptions::from_config(&system_cfg, &user_cfg);
    let autostart_enabled = autostart_enabled(&handle);
    let prevent_sleep_while_downloading =
        parse_boolish(user_cfg.get("prevent-sleep-while-downloading"), true);

    let engine_running = engine::engine_uptime().is_some();
    let snapshot = engine::startup_snapshot();

    // Live BT snapshot (None when torrent engine isn't initialized)
    let (bt_snapshot, tracker_urls) = if let Some(mgr) = engine::get_manager().await {
        let snap = mgr.bt_health_snapshot().await;
        let urls = mgr.list_active_tracker_urls().await;
        (snap, urls)
    } else {
        (None, Vec::new())
    };

    let want = |id: &str| {
        categories
            .as_ref()
            .map(|list| list.iter().any(|c| c == id))
            .unwrap_or(true)
    };

    let mut cats: Vec<HealthCategory> = Vec::new();

    if want("general") {
        cats.push(HealthCategory::from_checks(
            "general",
            check_general(&options, engine_running),
        ));
    }
    if want("network") {
        cats.push(HealthCategory::from_checks(
            "network",
            check_network(&options, slow).await,
        ));
    }
    if want("bittorrent") {
        cats.push(HealthCategory::from_checks(
            "bittorrent",
            check_bittorrent(&options, bt_snapshot.as_ref(), &tracker_urls, slow).await,
        ));
    }
    if want("disk") {
        cats.push(HealthCategory::from_checks(
            "disk",
            check_disk(&options, &user_cfg),
        ));
    }
    if want("system") {
        cats.push(HealthCategory::from_checks(
            "system",
            check_system(autostart_enabled, prevent_sleep_while_downloading),
        ));
    }
    if want("config") {
        cats.push(HealthCategory::from_checks(
            "config",
            check_config(&system_cfg, &user_cfg, snapshot.as_ref()),
        ));
    }
    if want("logs") {
        cats.push(HealthCategory::from_checks("logs", check_logs(&log_dir)));
    }
    if want("tools") && !cfg!(target_os = "android") {
        cats.push(HealthCategory::from_checks("tools", check_tools().await));
    }

    let overall_status = cats
        .iter()
        .map(|c| c.status)
        .fold(HealthStatus::Ok, HealthStatus::worst);

    Ok(HealthReport {
        generated_at_ms: SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
        engine_running,
        overall_status,
        categories: cats,
        log_path: log_dir.to_string_lossy().to_string(),
    })
}

// General

fn check_general(options: &EngineOptions, running: bool) -> Vec<HealthCheck> {
    let mut out = Vec::new();

    if running {
        let secs = engine::engine_uptime().map(|d| d.as_secs()).unwrap_or(0);
        out.push(
            HealthCheck::ok("engine-running", "Engine is running")
                .with_details(serde_json::json!({ "uptimeSeconds": secs })),
        );
    } else {
        out.push(HealthCheck::fail(
            "engine-running",
            "Engine is not running",
            Some(HealthFix::restart_engine()),
        ));
    }

    out.push(
        HealthCheck::ok("app-version", format!("Risuko {}", APP_VERSION))
            .with_details(serde_json::json!({ "version": APP_VERSION })),
    );

    let port = options.rpc_listen_port();
    let host = options.rpc_host();
    out.push(
        HealthCheck::ok("rpc-endpoint", format!("RPC bound to {}:{}", host, port))
            .with_details(serde_json::json!({ "host": host, "port": port })),
    );

    if options.rpc_secret().is_empty() && !is_loopback(&host) {
        out.push(HealthCheck::warn(
            "rpc-secret",
            "RPC secret is empty and host is not loopback",
            Some(HealthFix::open_pref("advanced")),
        ));
    } else {
        out.push(HealthCheck::ok(
            "rpc-secret",
            "RPC interface protected (loopback-only or secret set)",
        ));
    }

    out
}

fn is_loopback(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost" | "::1" | "[::1]")
}

fn parse_boolish(v: Option<&Value>, default: bool) -> bool {
    match v {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s.eq_ignore_ascii_case("true"),
        _ => default,
    }
}

// Network

async fn check_network(options: &EngineOptions, slow: bool) -> Vec<HealthCheck> {
    let mut out = Vec::new();

    let listen_port = options.get_u64("listen-port").unwrap_or(0);
    if (1..=65535).contains(&listen_port) {
        out.push(HealthCheck::ok(
            "listen-port",
            format!("BT listen port: {}", listen_port),
        ));
    } else {
        out.push(HealthCheck::warn(
            "listen-port",
            "BT listen port is not configured",
            Some(HealthFix::open_pref("advanced")),
        ));
    }

    let dht_port = options.get_u64("dht-listen-port").unwrap_or(0);
    if (1..=65535).contains(&dht_port) {
        out.push(HealthCheck::ok(
            "dht-listen-port",
            format!("DHT listen port: {}", dht_port),
        ));
    } else {
        out.push(HealthCheck::warn(
            "dht-listen-port",
            "DHT listen port is not configured",
            Some(HealthFix::open_pref("advanced")),
        ));
    }

    if options.bt_listen_v6() {
        out.push(HealthCheck::ok("ipv6", "IPv6 listener enabled"));
    } else {
        out.push(HealthCheck::skipped("ipv6", "IPv6 listener disabled"));
    }

    let proxy = options
        .get_str("all-proxy")
        .unwrap_or("")
        .trim()
        .to_string();
    if proxy.is_empty() {
        out.push(HealthCheck::skipped("proxy", "No proxy configured"));
    } else if !proxy.contains("://") {
        out.push(HealthCheck::fail(
            "proxy",
            format!("Proxy URL appears malformed: {}", proxy),
            Some(HealthFix::open_pref("advanced")),
        ));
    } else if slow {
        out.push(probe_proxy_reachability(&proxy).await);
    } else {
        out.push(HealthCheck::ok("proxy", format!("Proxy: {}", proxy)));
    }

    out
}

async fn probe_proxy_reachability(proxy_url: &str) -> HealthCheck {
    use risuko_http::{ClientBuilder, Method, Proxy};

    let proxy = match Proxy::all(proxy_url) {
        Ok(p) => p,
        Err(e) => {
            return HealthCheck::fail(
                "proxy",
                format!("Proxy parse error: {e}"),
                Some(HealthFix::open_pref("advanced")),
            );
        }
    };
    let client = match ClientBuilder::new()
        .proxy(proxy)
        .timeout(PROXY_PROBE_TIMEOUT)
        .connect_timeout(PROXY_PROBE_TIMEOUT)
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return HealthCheck::fail(
                "proxy",
                format!("Proxy client init failed: {e}"),
                Some(HealthFix::open_pref("advanced")),
            );
        }
    };
    match client.request(Method::HEAD, PROXY_PROBE_URL).send().await {
        Ok(resp) => {
            let status = resp.status();
            if status.is_success() || status.is_redirection() {
                HealthCheck::ok(
                    "proxy",
                    format!("Proxy reachable ({proxy_url}, HTTP {})", status.as_u16()),
                )
            } else {
                HealthCheck::warn(
                    "proxy",
                    format!(
                        "Proxy returned HTTP {} for probe ({proxy_url})",
                        status.as_u16()
                    ),
                    Some(HealthFix::open_pref("advanced")),
                )
            }
        }
        Err(e) => HealthCheck::fail(
            "proxy",
            format!("Proxy probe failed via {proxy_url}: {e}"),
            Some(HealthFix::open_pref("advanced")),
        ),
    }
}

// Bittorrent

async fn check_bittorrent(
    options: &EngineOptions,
    bt: Option<&BtHealthSnapshot>,
    tracker_urls: &[String],
    slow: bool,
) -> Vec<HealthCheck> {
    let mut out = Vec::new();

    // DHT needs ~30s after engine start to bootstrap. Treat the bootstrap
    // window as "probing" rather than a warning. (UPnP uses its own attempt
    // counter rather than a fixed window)
    const BOOTSTRAP_WINDOW_SECS: u64 = 60;
    let uptime_secs = engine::engine_uptime().map(|d| d.as_secs()).unwrap_or(0);
    let bootstrapping = uptime_secs < BOOTSTRAP_WINDOW_SECS;

    out.push(HealthCheck::ok(
        "encryption-policy",
        format!("Encryption: {}", options.bt_encryption_policy()),
    ));

    // UPnP, combine config flag with live mapping count when available
    let upnp_cfg = options.bt_enable_upnp();
    match (upnp_cfg, bt) {
        (false, _) => out.push(HealthCheck::skipped(
            "upnp",
            "UPnP port forwarding disabled",
        )),
        (true, None) => out.push(HealthCheck::skipped(
            "upnp",
            "UPnP enabled (BT session not started yet)",
        )),
        (true, Some(snap)) if snap.upnp_mappings > 0 => out.push(
            HealthCheck::ok(
                "upnp",
                format!("UPnP active — {} mapping(s) confirmed", snap.upnp_mappings),
            )
            .with_details(serde_json::json!({
                "mappings": snap.upnp_mappings,
                "listenPort": snap.listen_port,
            })),
        ),
        (true, Some(snap)) if snap.upnp_attempts == 0 => out.push(HealthCheck::skipped(
            "upnp",
            "UPnP negotiating with router…",
        )),
        (true, Some(_)) => out.push(HealthCheck::warn(
            "upnp",
            "UPnP enabled but no IGD responded — router may not support UPnP or has it disabled",
            None,
        )),
    }

    // LSD, combine config flag with live handle status.
    let lsd_cfg = options.bt_enable_lsd();
    match (lsd_cfg, bt) {
        (false, _) => out.push(HealthCheck::skipped(
            "lsd",
            "Local Service Discovery disabled",
        )),
        (true, None) => out.push(HealthCheck::skipped(
            "lsd",
            "LSD enabled (BT session not started yet)",
        )),
        (true, Some(snap)) if snap.lsd_active => {
            out.push(HealthCheck::ok("lsd", "Local Service Discovery active"))
        }
        (true, Some(_)) => out.push(HealthCheck::warn(
            "lsd",
            "LSD enabled but listener failed to start (no multicast?)",
            None,
        )),
    }

    if let Some(snap) = bt {
        out.push(
            HealthCheck::ok(
                "bt-session",
                format!(
                    "BT listening on port {} — {} torrent(s) loaded",
                    snap.listen_port, snap.torrents
                ),
            )
            .with_details(serde_json::json!({
                "listenPort": snap.listen_port,
                "torrents": snap.torrents,
            })),
        );

        let dht_check = if !snap.dht_active {
            HealthCheck::skipped("dht", "DHT disabled")
        } else if snap.dht_nodes == 0 && bootstrapping {
            HealthCheck::skipped("dht", "DHT bootstrapping…")
        } else if snap.dht_nodes == 0 {
            HealthCheck::warn(
                "dht",
                "DHT enabled but no nodes in routing table (UDP blocked?)",
                None,
            )
        } else if snap.dht_nodes < 8 {
            HealthCheck::warn(
                "dht",
                format!("DHT routing table sparse: {} node(s)", snap.dht_nodes),
                None,
            )
        } else {
            HealthCheck::ok(
                "dht",
                format!("DHT healthy — {} nodes in routing table", snap.dht_nodes),
            )
        };
        out.push(dht_check.with_details(serde_json::json!({
            "active": snap.dht_active,
            "nodes": snap.dht_nodes,
        })));
    }

    let max_peers = options.bt_max_peers_per_torrent();
    let max_outstanding = options.bt_max_outstanding_per_peer();
    out.push(
        HealthCheck::ok(
            "bt-tuning",
            format!(
                "Max peers/torrent: {}, max outstanding/peer: {}",
                fmt_opt(max_peers, "default"),
                fmt_opt(max_outstanding, "default"),
            ),
        )
        .with_details(serde_json::json!({
            "maxPeersPerTorrent": max_peers,
            "maxOutstandingPerPeer": max_outstanding,
        })),
    );

    // Tracker reachability, only when there are active torrents and the
    // caller opted in to slow probes (each probe is 3s timeout-bound)
    if tracker_urls.is_empty() {
        out.push(HealthCheck::skipped(
            "trackers",
            "No active BT torrents — tracker probe skipped",
        ));
    } else if !slow {
        out.push(
            HealthCheck::skipped(
                "trackers",
                format!(
                    "{} unique tracker(s) configured — enable deep probes to test reachability",
                    tracker_urls.len()
                ),
            )
            .with_details(serde_json::json!({ "trackers": tracker_urls.len() })),
        );
    } else {
        out.push(probe_trackers(tracker_urls).await);
    }

    out
}

async fn probe_trackers(urls: &[String]) -> HealthCheck {
    use risuko_http::{ClientBuilder, Method};
    use tokio::task::JoinSet;

    // Cap concurrent probes to avoid hammering the network in trackers-heavy
    // setups. 8 in-flight is a sensible default
    const MAX_CONCURRENCY: usize = 8;
    const PER_TRACKER_TIMEOUT: Duration = Duration::from_secs(3);

    let client = match ClientBuilder::new()
        .timeout(PER_TRACKER_TIMEOUT)
        .connect_timeout(PER_TRACKER_TIMEOUT)
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return HealthCheck::fail("trackers", format!("Probe client init failed: {e}"), None);
        }
    };

    #[derive(Default)]
    struct Counts {
        ok: usize,
        warn: usize,
        fail: usize,
        skipped: usize,
    }

    async fn probe_one(client: risuko_http::Client, url: String) -> &'static str {
        let lower = url.to_ascii_lowercase();
        if lower.starts_with("udp://") || lower.starts_with("ws://") || lower.starts_with("wss://")
        {
            return "skipped";
        }
        if !(lower.starts_with("http://") || lower.starts_with("https://")) {
            return "skipped";
        }
        match client.request(Method::HEAD, &url).send().await {
            Ok(resp) => {
                let s = resp.status();
                if s.is_success() || s.is_redirection() || s.as_u16() == 400 {
                    // for HEAD-on-announce (missing query)
                    "ok"
                } else {
                    "warn"
                }
            }
            Err(_) => "fail",
        }
    }

    let mut counts = Counts::default();
    let mut iter = urls.iter().cloned();
    let mut set: JoinSet<&'static str> = JoinSet::new();
    // Prime the pump
    for _ in 0..MAX_CONCURRENCY {
        if let Some(url) = iter.next() {
            let c = client.clone();
            set.spawn(async move { probe_one(c, url).await });
        } else {
            break;
        }
    }
    while let Some(joined) = set.join_next().await {
        let kind = joined.unwrap_or("fail");
        match kind {
            "ok" => counts.ok += 1,
            "warn" => counts.warn += 1,
            "fail" => counts.fail += 1,
            _ => counts.skipped += 1,
        }
        if let Some(url) = iter.next() {
            let c = client.clone();
            set.spawn(async move { probe_one(c, url).await });
        }
    }

    let total = counts.ok + counts.warn + counts.fail + counts.skipped;
    let details = serde_json::json!({
        "total": total,
        "ok": counts.ok,
        "warn": counts.warn,
        "fail": counts.fail,
        "skipped": counts.skipped,
    });
    let msg = format!(
        "Trackers: {} ok, {} warn, {} fail, {} skipped (of {})",
        counts.ok, counts.warn, counts.fail, counts.skipped, total
    );
    let check = if counts.fail > 0 && counts.ok == 0 {
        HealthCheck::fail("trackers", msg, None)
    } else if counts.fail > 0 || counts.warn > 0 {
        HealthCheck::warn("trackers", msg, None)
    } else if counts.ok == 0 {
        HealthCheck::skipped("trackers", msg)
    } else {
        HealthCheck::ok("trackers", msg)
    };
    check.with_details(details)
}

fn fmt_opt<T: std::fmt::Display>(v: Option<T>, fallback: &str) -> String {
    v.map(|x| x.to_string()).unwrap_or_else(|| fallback.into())
}

// Disk

fn check_disk(
    options: &EngineOptions,
    user_cfg: &serde_json::Map<String, Value>,
) -> Vec<HealthCheck> {
    let mut out = Vec::new();

    let dir = options.dir();
    let dir_check = probe_dir("download-dir", &dir);
    out.push(dir_check);

    let alloc = options.get_str("file-allocation").unwrap_or("falloc");
    out.push(HealthCheck::ok(
        "file-allocation",
        format!("File allocation: {}", alloc),
    ));

    // Each entry in the recent-saved-paths history
    if let Some(arr) = user_cfg
        .get("recent-saved-paths")
        .and_then(|v| v.as_array())
    {
        for (i, v) in arr.iter().enumerate() {
            let Some(path) = v.as_str() else { continue };
            // Skip the primary dir (already checked above)
            if path == dir.as_str() {
                continue;
            }
            let id = format!("recent-dir-{}", i);
            // Demote failures to warnings here
            let mut probed = probe_dir(&id, path);
            if probed.status == HealthStatus::Fail {
                probed.status = HealthStatus::Warn;
            }
            out.push(probed);
        }
    }

    out
}

fn probe_dir(id: &str, path: &str) -> HealthCheck {
    if path.is_empty() {
        return HealthCheck::fail(
            id,
            "Download directory is not configured",
            Some(HealthFix::open_pref("basic")),
        );
    }
    let p = Path::new(path);
    if !p.exists() {
        return HealthCheck::fail(
            id,
            format!("Path does not exist: {}", path),
            Some(HealthFix::open_pref("basic")),
        );
    }
    if !p.is_dir() {
        return HealthCheck::fail(
            id,
            format!("Path is not a directory: {}", path),
            Some(HealthFix::open_pref("basic")),
        );
    }

    // Touch test: create, write, and auto-remove a unique temp file
    let writable = tempfile::NamedTempFile::new_in(p)
        .and_then(|mut probe| {
            probe.write_all(b"probe")?;
            probe.flush()
        })
        .is_ok();
    if !writable {
        return HealthCheck::fail(
            id,
            format!("Directory is not writable: {}", path),
            Some(HealthFix::open_pref("basic")),
        );
    }

    let (free, total) = disk_space(p);
    let mut details = serde_json::json!({ "path": path });
    if let (Some(free_b), Some(total_b)) = (free, total) {
        details["freeBytes"] = serde_json::json!(free_b);
        details["totalBytes"] = serde_json::json!(total_b);

        let pct = if total_b > 0 {
            (free_b as f64 / total_b as f64) * 100.0
        } else {
            100.0
        };
        let msg = format!(
            "{} — {} free of {} ({:.1}%)",
            path,
            human_bytes(free_b),
            human_bytes(total_b),
            pct
        );
        if free_b < 1024 * 1024 * 1024 || pct < 5.0 {
            return HealthCheck::fail(id, msg, Some(HealthFix::open_pref("basic")))
                .with_details(details);
        }
        if free_b < 5 * 1024 * 1024 * 1024 || pct < 10.0 {
            return HealthCheck::warn(id, msg, None).with_details(details);
        }
        return HealthCheck::ok(id, msg).with_details(details);
    }

    HealthCheck::ok(id, format!("{} writable (free space unknown)", path)).with_details(details)
}

fn disk_space(p: &Path) -> (Option<u64>, Option<u64>) {
    // `fs4` wraps `statvfs` on unix and `GetDiskFreeSpaceExW` on windows
    let free = fs4::available_space(p).ok();
    let total = fs4::total_space(p).ok();
    (free, total)
}

fn human_bytes(b: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;
    if b >= TB {
        format!("{:.1} TB", b as f64 / TB as f64)
    } else if b >= GB {
        format!("{:.1} GB", b as f64 / GB as f64)
    } else if b >= MB {
        format!("{:.1} MB", b as f64 / MB as f64)
    } else if b >= KB {
        format!("{:.1} KB", b as f64 / KB as f64)
    } else {
        format!("{} B", b)
    }
}

// System

fn check_system(autostart: bool, prevent_sleep_while_downloading: bool) -> Vec<HealthCheck> {
    let mut out = Vec::new();
    if autostart {
        out.push(HealthCheck::ok("autostart", "Launch at login enabled"));
    } else {
        out.push(HealthCheck::skipped(
            "autostart",
            "Launch at login disabled",
        ));
    }

    if !prevent_sleep_while_downloading {
        out.push(HealthCheck::skipped(
            "sleep-inhibit",
            "Sleep inhibit disabled in preferences",
        ));
    } else if sleep_inhibit_active() {
        out.push(HealthCheck::ok("sleep-inhibit", "Sleep inhibit active"));
    } else {
        out.push(HealthCheck::skipped(
            "sleep-inhibit",
            "Sleep inhibit enabled (will activate during downloads)",
        ));
    }
    out
}

fn autostart_enabled(handle: &AppHandle) -> bool {
    #[cfg(target_os = "android")]
    {
        let _ = handle;
        false
    }
    #[cfg(not(target_os = "android"))]
    {
        handle.autolaunch().is_enabled().unwrap_or(false)
    }
}

// Config

fn check_config(
    system: &serde_json::Map<String, Value>,
    user: &serde_json::Map<String, Value>,
    snapshot: Option<&HashMap<String, Value>>,
) -> Vec<HealthCheck> {
    let mut out = Vec::new();

    let Some(snap) = snapshot else {
        out.push(HealthCheck::skipped(
            "startup-key-drift",
            "Engine has not started yet",
        ));
        return out;
    };

    let mut drifted: Vec<String> = Vec::new();
    for key in engine::STARTUP_ONLY_KEYS {
        let current = system
            .get(*key)
            .or_else(|| user.get(*key))
            .cloned()
            .unwrap_or(Value::Null);
        let prev = snap.get(*key).cloned().unwrap_or(Value::Null);
        if current != prev {
            drifted.push((*key).to_string());
        }
    }

    if drifted.is_empty() {
        out.push(HealthCheck::ok(
            "startup-key-drift",
            "All startup-only keys match the running engine",
        ));
    } else {
        out.push(
            HealthCheck::warn(
                "startup-key-drift",
                format!(
                    "{} startup-only key(s) changed since the engine started — restart to apply",
                    drifted.len()
                ),
                Some(HealthFix::restart_engine()),
            )
            .with_details(serde_json::json!({ "keys": drifted })),
        );
    }

    out
}

// Logs

fn check_logs(log_dir: &Path) -> Vec<HealthCheck> {
    let mut out = Vec::new();

    if !log_dir.exists() {
        out.push(HealthCheck::warn(
            "log-dir",
            format!("Log directory missing: {}", log_dir.display()),
            None,
        ));
        return out;
    }
    out.push(HealthCheck::ok(
        "log-dir",
        format!("Log directory: {}", log_dir.display()),
    ));

    // tracing-appender names today's file `risuko.log.YYYY-MM-DD`. Find the
    // most recently modified file matching that prefix
    let latest = std::fs::read_dir(log_dir)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("risuko.log"))
        .filter_map(|e| {
            let path = e.path();
            let mtime = e.metadata().and_then(|m| m.modified()).ok()?;
            Some((path, mtime))
        })
        .max_by_key(|(_, m)| *m);

    let Some((path, _)) = latest else {
        out.push(HealthCheck::skipped("log-file", "No log files yet"));
        return out;
    };

    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let (errs, warns) = tail_count_levels(&path).unwrap_or((0, 0));

    let details = serde_json::json!({
        "path": path.to_string_lossy(),
        "sizeBytes": size,
        "errorCount": errs,
        "warnCount": warns,
    });

    let msg = format!(
        "{} — {} (errors: {}, warnings: {})",
        path.file_name().unwrap_or_default().to_string_lossy(),
        human_bytes(size),
        errs,
        warns
    );

    let check = if errs > 0 || warns > 5 {
        HealthCheck::warn("log-file", msg, Some(HealthFix::open_log_dir()))
    } else {
        HealthCheck::ok("log-file", msg)
    };
    out.push(check.with_details(details));
    out
}

// Tools

async fn check_tools() -> Vec<HealthCheck> {
    let mut out = Vec::new();

    match media::check_yt_dlp_available().await {
        Ok(()) => {
            // Capture the version string for display
            let version = tokio::process::Command::new("yt-dlp")
                .arg("--version")
                .output()
                .await
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|| "unknown".to_string());
            out.push(
                HealthCheck::ok("yt-dlp", format!("yt-dlp available ({})", version))
                    .with_details(serde_json::json!({ "version": version })),
            );
        }
        Err(_) => {
            out.push(HealthCheck::fail(
                "yt-dlp",
                "yt-dlp not found in PATH — media-site downloads will fail",
                None,
            ));
        }
    }

    // ffmpeg is optional but enables merging of separate video+audio streams
    // Without it, media downloads fall back to a single pre-muxed stream
    if let Some(ffmpeg_path) = media::find_ffmpeg().await {
        let version = tokio::process::Command::new(&ffmpeg_path)
            .arg("-version")
            .output()
            .await
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| s.lines().next().map(|l| l.trim().to_string()))
            .unwrap_or_else(|| "unknown".to_string());
        out.push(
            HealthCheck::ok("ffmpeg", format!("ffmpeg available ({version})"))
                .with_details(serde_json::json!({ "version": version })),
        );
    } else {
        out.push(HealthCheck::warn(
            "ffmpeg",
            "ffmpeg not found in PATH — media downloads fall back to single-file \
             quality (no video+audio merge)",
            None,
        ));
    }

    out
}

fn tail_count_levels(path: &Path) -> std::io::Result<(usize, usize)> {
    let mut f = std::fs::File::open(path)?;
    let len = f.metadata()?.len();
    let start = len.saturating_sub(LOG_TAIL_BYTES);
    f.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::with_capacity((len - start) as usize);
    f.read_to_end(&mut buf)?;
    let text = String::from_utf8_lossy(&buf);

    let mut errors = 0usize;
    let mut warnings = 0usize;
    for line in text.lines() {
        if line.contains(" ERROR ") {
            errors += 1;
        } else if line.contains(" WARN ") {
            warnings += 1;
        }
    }
    Ok((errors, warnings))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_bytes_unit_boundaries() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(1024), "1.0 KB");
        assert_eq!(human_bytes(1024 * 1024), "1.0 MB");
        assert_eq!(human_bytes(1024 * 1024 * 1024), "1.0 GB");
    }

    #[test]
    fn worst_status_picks_highest_rank() {
        assert_eq!(
            HealthStatus::Ok.worst(HealthStatus::Warn),
            HealthStatus::Warn
        );
        assert_eq!(
            HealthStatus::Fail.worst(HealthStatus::Warn),
            HealthStatus::Fail
        );
        assert_eq!(
            HealthStatus::Skipped.worst(HealthStatus::Ok),
            HealthStatus::Skipped
        );
    }

    #[test]
    fn category_status_rolls_up_to_worst() {
        let cat = HealthCategory::from_checks(
            "x",
            vec![
                HealthCheck::ok("a", "ok"),
                HealthCheck::warn("b", "warn", None),
            ],
        );
        assert_eq!(cat.status, HealthStatus::Warn);
    }

    #[test]
    fn tail_count_levels_counts_warn_and_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.log");
        let body = "2026-05-03 INFO  ok\n2026-05-03 WARN  bad\n2026-05-03 ERROR boom\n2026-05-03 ERROR boom2\n";
        std::fs::write(&path, body).unwrap();
        let (errs, warns) = tail_count_levels(&path).unwrap();
        assert_eq!(errs, 2);
        assert_eq!(warns, 1);
    }
}
