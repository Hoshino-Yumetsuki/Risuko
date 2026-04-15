use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Map, Value};

use risuko_engine::config::defaults;
use risuko_engine::engine::events::EventBroadcaster;
use risuko_engine::engine::manager::TaskManager;
use risuko_engine::engine::options::EngineOptions;
use risuko_engine::engine::rpc::RpcServer;

use crate::progress::{self, format_size, format_size_speed};
use crate::rpc_client::RpcClient;
use crate::{
    ConfigAction, ConfigCommand, DownloadArgs, GidArgs, PauseArgs, RemoveArgs, ResumeArgs, RpcArgs,
    RssAction, RssCommand, ServeArgs, StatusArgs,
};

// Download

pub async fn download(args: DownloadArgs) -> Result<(), Box<dyn std::error::Error>> {
    let client = RpcClient::new(args.rpc_port, args.rpc_secret.clone());
    let mut headless_engine = None;

    if !client.is_engine_running().await {
        eprintln!("No running Risuko instance found. Starting headless engine...");
        let engine = start_headless_engine(args.rpc_port).await?;
        headless_engine = Some(engine);
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }

    let result = do_download(&client, &args).await;

    if let Some(engine) = headless_engine {
        engine.shutdown().await;
    }

    result
}

async fn do_download(
    client: &RpcClient,
    args: &DownloadArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut options = serde_json::Map::new();

    options.insert("split".into(), json!(args.threads.to_string()));

    if let Some(ref dir) = args.dir {
        options.insert("dir".into(), json!(dir));
    }
    if let Some(ref out) = args.out {
        options.insert("out".into(), json!(out));
    }
    if let Some(ref ua) = args.user_agent {
        options.insert("user-agent".into(), json!(ua));
    }
    if let Some(ref proxy) = args.proxy {
        options.insert("all-proxy".into(), json!(proxy));
    }
    if let Some(ref referer) = args.referer {
        options.insert("referer".into(), json!(referer));
    }
    if let Some(ref cookie) = args.cookie {
        if !args
            .headers
            .iter()
            .any(|h| h.to_lowercase().starts_with("cookie:"))
        {
            if let Some(arr) = options
                .entry("header".to_string())
                .or_insert_with(|| json!([]))
                .as_array_mut()
            {
                arr.push(json!(format!("Cookie: {}", cookie)));
            }
        }
    }
    if let Some(ratio) = args.seed_ratio {
        options.insert("seed-ratio".into(), json!(ratio.to_string()));
    }
    if let Some(time) = args.seed_time {
        options.insert("seed-time".into(), json!(time.to_string()));
    }

    if !args.headers.is_empty() {
        let existing = options
            .get("header")
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default();
        let mut all_headers: Vec<Value> = existing;
        for h in &args.headers {
            all_headers.push(json!(h));
        }
        options.insert("header".into(), json!(all_headers));
    }

    let is_torrent = args.url.ends_with(".torrent") && std::path::Path::new(&args.url).exists();

    let gid = if is_torrent {
        let torrent_data = std::fs::read(&args.url)?;
        let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &torrent_data);
        let result = client
            .call(
                "risuko.addTorrent",
                vec![json!(b64), json!([]), json!(options)],
            )
            .await?;
        result.as_str().unwrap_or("").to_string()
    } else {
        let result = client
            .call("risuko.addUri", vec![json!([&args.url]), json!(options)])
            .await?;
        result.as_str().unwrap_or("").to_string()
    };

    if gid.is_empty() {
        return Err("Failed to get task GID from engine".into());
    }

    if !args.json {
        eprintln!("Download started (GID: {})", gid);
    }

    progress::watch_download(&client, &gid, args.json).await
}

// Status

pub async fn status(args: StatusArgs) -> Result<(), Box<dyn std::error::Error>> {
    let client = RpcClient::new(args.rpc_port, args.rpc_secret.clone());
    require_engine(&client).await?;

    if let Some(ref gid) = args.gid {
        let status = client.call("risuko.tellStatus", vec![json!(gid)]).await?;
        if args.json {
            println!("{}", serde_json::to_string_pretty(&status)?);
        } else {
            print_task_detail(&status);
        }
    } else {
        let all_keys = json!([
            "gid",
            "status",
            "totalLength",
            "completedLength",
            "downloadSpeed",
            "uploadSpeed",
            "files"
        ]);
        let active = client
            .call("risuko.tellActive", vec![all_keys.clone()])
            .await?;
        let waiting = client
            .call(
                "risuko.tellWaiting",
                vec![json!(0), json!(100), all_keys.clone()],
            )
            .await?;
        let stopped = client
            .call("risuko.tellStopped", vec![json!(0), json!(100), all_keys])
            .await?;

        let mut tasks: Vec<Value> = Vec::new();
        for list in [&active, &waiting, &stopped] {
            if let Some(arr) = list.as_array() {
                tasks.extend(arr.iter().cloned());
            }
        }

        if args.json {
            println!("{}", serde_json::to_string_pretty(&tasks)?);
        } else if tasks.is_empty() {
            println!("No downloads.");
        } else {
            print_task_table(&tasks);
        }
    }

    Ok(())
}

// Pause / Resume / Remove

pub async fn pause(args: PauseArgs) -> Result<(), Box<dyn std::error::Error>> {
    let client = RpcClient::new(args.rpc_port, args.rpc_secret.clone());
    require_engine(&client).await?;
    client.call("risuko.pause", vec![json!(args.gid)]).await?;
    println!("Paused: {}", args.gid);
    Ok(())
}

pub async fn resume(args: ResumeArgs) -> Result<(), Box<dyn std::error::Error>> {
    let client = RpcClient::new(args.rpc_port, args.rpc_secret.clone());
    require_engine(&client).await?;
    client.call("risuko.unpause", vec![json!(args.gid)]).await?;
    println!("Resumed: {}", args.gid);
    Ok(())
}

pub async fn remove(args: RemoveArgs) -> Result<(), Box<dyn std::error::Error>> {
    let client = RpcClient::new(args.rpc_port, args.rpc_secret.clone());
    require_engine(&client).await?;
    for gid in &args.gids {
        match client.call("risuko.remove", vec![json!(gid)]).await {
            Ok(_) => println!("Removed: {gid}"),
            Err(e) => eprintln!("Failed to remove {gid}: {e}"),
        }
    }
    Ok(())
}

// Pause All / Resume All

pub async fn pause_all(args: RpcArgs) -> Result<(), Box<dyn std::error::Error>> {
    let client = RpcClient::new(args.rpc_port, args.rpc_secret.clone());
    require_engine(&client).await?;
    client.call("risuko.pauseAll", vec![]).await?;
    println!("All downloads paused.");
    Ok(())
}

pub async fn resume_all(args: RpcArgs) -> Result<(), Box<dyn std::error::Error>> {
    let client = RpcClient::new(args.rpc_port, args.rpc_secret.clone());
    require_engine(&client).await?;
    client.call("risuko.unpauseAll", vec![]).await?;
    println!("All downloads resumed.");
    Ok(())
}

// Global Stat

pub async fn global_stat(args: RpcArgs) -> Result<(), Box<dyn std::error::Error>> {
    let client = RpcClient::new(args.rpc_port, args.rpc_secret.clone());
    require_engine(&client).await?;
    let stat = client.call("risuko.getGlobalStat", vec![]).await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&stat)?);
    } else {
        let dl = parse_num(&stat, "downloadSpeed");
        let ul = parse_num(&stat, "uploadSpeed");
        let active = parse_num(&stat, "numActive");
        let waiting = parse_num(&stat, "numWaiting");
        let stopped = parse_num(&stat, "numStopped");

        println!(
            "Download: {}   Upload: {}",
            format_size_speed(dl),
            format_size_speed(ul)
        );
        println!(
            "Active: {}   Waiting: {}   Stopped: {}",
            active, waiting, stopped
        );
    }

    Ok(())
}

// Files / Peers

pub async fn files(args: GidArgs) -> Result<(), Box<dyn std::error::Error>> {
    let client = RpcClient::new(args.rpc_port, args.rpc_secret.clone());
    require_engine(&client).await?;
    let result = client
        .call("risuko.getFiles", vec![json!(args.gid)])
        .await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else if let Some(files) = result.as_array() {
        println!("{:<4} {:<10} {:<10} {}", "Idx", "Size", "Done", "Path");
        println!("{}", "-".repeat(60));
        for (i, f) in files.iter().enumerate() {
            let length = parse_num(f, "length");
            let completed = parse_num(f, "completedLength");
            let path = f.get("path").and_then(|v| v.as_str()).unwrap_or("-");
            println!(
                "{:<4} {:<10} {:<10} {}",
                i + 1,
                format_size(length),
                format_size(completed),
                path,
            );
        }
    }

    Ok(())
}

pub async fn peers(args: GidArgs) -> Result<(), Box<dyn std::error::Error>> {
    let client = RpcClient::new(args.rpc_port, args.rpc_secret.clone());
    require_engine(&client).await?;
    let result = client
        .call("risuko.getPeers", vec![json!(args.gid)])
        .await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else if let Some(peers) = result.as_array() {
        if peers.is_empty() {
            println!("No peers.");
        } else {
            println!(
                "{:<22} {:<12} {:<12} {}",
                "IP", "DL Speed", "UL Speed", "Client"
            );
            println!("{}", "-".repeat(60));
            for p in peers {
                let ip = p.get("ip").and_then(|v| v.as_str()).unwrap_or("-");
                let port = p.get("port").and_then(|v| v.as_str()).unwrap_or("");
                let dl = parse_num(p, "downloadSpeed");
                let ul = parse_num(p, "uploadSpeed");
                let peer_id = p.get("peerId").and_then(|v| v.as_str()).unwrap_or("-");
                println!(
                    "{:<22} {:<12} {:<12} {}",
                    format!("{}:{}", ip, port),
                    format_size_speed(dl),
                    format_size_speed(ul),
                    peer_id,
                );
            }
        }
    }

    Ok(())
}

// Purge

pub async fn purge(args: RpcArgs) -> Result<(), Box<dyn std::error::Error>> {
    let client = RpcClient::new(args.rpc_port, args.rpc_secret.clone());
    require_engine(&client).await?;
    client.call("risuko.purgeDownloadResult", vec![]).await?;
    println!("Purged completed/error/removed download results.");
    Ok(())
}

// Config

pub async fn config(cmd: ConfigCommand) -> Result<(), Box<dyn std::error::Error>> {
    match cmd.action {
        ConfigAction::Get { key } => {
            let config_dir = get_config_dir();
            let system = load_config(&config_dir.join("system.json"), defaults::system_defaults());
            let user = load_config(&config_dir.join("user.json"), defaults::user_defaults());
            // Merge: user overrides system
            let mut merged = system;
            for (k, v) in user {
                merged.insert(k, v);
            }
            match merged.get(&key) {
                Some(val) => println!("{}", serde_json::to_string_pretty(val)?),
                None => println!("Key '{}' not found", key),
            }
            Ok(())
        }
        ConfigAction::Set { key, value } => {
            let config_dir = get_config_dir();
            let path = config_dir.join("user.json");
            let mut config = load_config(&path, Map::new());
            let parsed: Value =
                serde_json::from_str(&value).unwrap_or_else(|_| Value::String(value.clone()));
            config.insert(key.clone(), parsed);
            std::fs::create_dir_all(&config_dir)?;
            std::fs::write(&path, serde_json::to_string_pretty(&config)?)?;
            println!("Set {} = {}", key, value);
            Ok(())
        }
        ConfigAction::List { json } => {
            let config_dir = get_config_dir();
            let system = load_config(&config_dir.join("system.json"), defaults::system_defaults());
            let user = load_config(&config_dir.join("user.json"), defaults::user_defaults());
            let mut merged = system;
            for (k, v) in user {
                merged.insert(k, v);
            }
            if json {
                println!("{}", serde_json::to_string_pretty(&merged)?);
            } else {
                let mut keys: Vec<&String> = merged.keys().collect();
                keys.sort();
                for k in keys {
                    println!("{}: {}", k, merged[k]);
                }
            }
            Ok(())
        }
    }
}

// RSS

pub async fn rss(cmd: RssCommand) -> Result<(), Box<dyn std::error::Error>> {
    match cmd.action {
        RssAction::Add {
            url,
            rpc_port,
            rpc_secret,
        } => {
            let client = RpcClient::new(rpc_port, rpc_secret);
            require_engine(&client).await?;
            let result = client.call("risuko.addRssFeed", vec![json!(url)]).await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
            Ok(())
        }
        RssAction::List {
            rpc_port,
            rpc_secret,
            json,
        } => {
            let client = RpcClient::new(rpc_port, rpc_secret);
            require_engine(&client).await?;
            let result = client.call("risuko.getRssFeeds", vec![]).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else if let Some(feeds) = result.as_array() {
                if feeds.is_empty() {
                    println!("No RSS feeds.");
                } else {
                    for feed in feeds {
                        let id = feed.get("id").and_then(|v| v.as_str()).unwrap_or("-");
                        let url = feed.get("url").and_then(|v| v.as_str()).unwrap_or("-");
                        let title = feed.get("title").and_then(|v| v.as_str()).unwrap_or("-");
                        println!("[{}] {} ({})", id, title, url);
                    }
                }
            }
            Ok(())
        }
        RssAction::Refresh {
            rpc_port,
            rpc_secret,
        } => {
            let client = RpcClient::new(rpc_port, rpc_secret);
            require_engine(&client).await?;
            client.call("risuko.refreshAllRssFeeds", vec![]).await?;
            println!("RSS feeds refreshed.");
            Ok(())
        }
        RssAction::Remove {
            id,
            rpc_port,
            rpc_secret,
        } => {
            let client = RpcClient::new(rpc_port, rpc_secret);
            require_engine(&client).await?;
            client.call("risuko.removeRssFeed", vec![json!(id)]).await?;
            println!("Removed RSS feed: {}", id);
            Ok(())
        }
    }
}

// Serve (headless engine)

pub async fn serve(args: ServeArgs) -> Result<(), Box<dyn std::error::Error>> {
    eprintln!("Starting Risuko engine on port {}...", args.rpc_port);
    let engine = start_headless_engine(args.rpc_port).await?;
    eprintln!("Risuko engine running. Press Ctrl+C to stop.");

    tokio::signal::ctrl_c().await?;
    eprintln!("\nShutting down...");
    engine.shutdown().await;
    Ok(())
}

// Shutdown

pub async fn shutdown(args: RpcArgs) -> Result<(), Box<dyn std::error::Error>> {
    let client = RpcClient::new(args.rpc_port, args.rpc_secret.clone());
    require_engine(&client).await?;
    client.call("risuko.shutdown", vec![]).await?;
    println!("Shutdown request sent.");
    Ok(())
}

// Headless engine (embedded)

struct HeadlessEngine {
    manager: Arc<TaskManager>,
    rpc_server: RpcServer,
    progress_task: tokio::task::JoinHandle<()>,
    auto_save_task: tokio::task::JoinHandle<()>,
}

impl HeadlessEngine {
    async fn shutdown(mut self) {
        self.progress_task.abort();
        self.auto_save_task.abort();
        self.rpc_server.stop();
        self.manager.shutdown().await;
        tracing::info!("Headless engine stopped");
    }
}

async fn start_headless_engine(
    rpc_port: u16,
) -> Result<HeadlessEngine, Box<dyn std::error::Error>> {
    let config_dir = get_config_dir();
    std::fs::create_dir_all(&config_dir)?;

    let system_config = load_config(&config_dir.join("system.json"), defaults::system_defaults());
    let user_config = load_config(&config_dir.join("user.json"), defaults::user_defaults());

    let mut options = EngineOptions::from_config(&system_config, &user_config);
    options.set("rpc-listen-port".into(), Value::from(rpc_port));

    let dir = options.dir();
    if !dir.is_empty() {
        std::fs::create_dir_all(&dir).ok();
    }

    let events = EventBroadcaster::default();
    let rpc_host = options.rpc_host();
    let rpc_secret = options.rpc_secret();

    tracing::info!("Starting headless engine on port {}", rpc_port);

    let manager = Arc::new(
        TaskManager::new(&config_dir, options, events.clone())
            .await
            .map_err(|e| format!("Failed to create task manager: {}", e))?,
    );

    let (rpc_shutdown_tx, _rpc_shutdown_rx) = tokio::sync::mpsc::channel::<()>(1);

    let mut rpc_server = RpcServer::new(
        rpc_host,
        rpc_port,
        rpc_secret,
        manager.clone(),
        events.clone(),
        rpc_shutdown_tx,
    );
    rpc_server
        .start()
        .await
        .map_err(|e| format!("Failed to start RPC server: {}", e))?;

    let mgr_progress = manager.clone();
    let progress_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
        loop {
            interval.tick().await;
            mgr_progress.update_progress().await;
        }
    });

    let mgr_save = manager.clone();
    let auto_save_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            interval.tick().await;
            if let Err(e) = mgr_save.save_session().await {
                tracing::warn!("Auto-save session failed: {}", e);
            }
        }
    });

    Ok(HeadlessEngine {
        manager,
        rpc_server,
        progress_task,
        auto_save_task,
    })
}

// Helpers

async fn require_engine(client: &RpcClient) -> Result<(), Box<dyn std::error::Error>> {
    if !client.is_engine_running().await {
        return Err(
            "No Risuko instance running. Start the app or use `risuko serve` first.".into(),
        );
    }
    Ok(())
}

fn get_config_dir() -> PathBuf {
    dirs::config_dir()
        .map(|d| d.join("dev.risuko.app"))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn load_config(path: &std::path::Path, defaults: Map<String, Value>) -> Map<String, Value> {
    if let Ok(data) = std::fs::read_to_string(path) {
        if let Ok(Value::Object(mut map)) = serde_json::from_str(&data) {
            for (k, v) in &defaults {
                if !map.contains_key(k) {
                    map.insert(k.clone(), v.clone());
                }
            }
            return map;
        }
    }
    defaults
}

fn parse_num(val: &Value, key: &str) -> u64 {
    val.get(key)
        .and_then(|v| v.as_str().and_then(|s| s.parse().ok()).or(v.as_u64()))
        .unwrap_or(0)
}

fn extract_name(task: &Value) -> String {
    task.get("files")
        .and_then(|f| f.as_array())
        .and_then(|arr| arr.first())
        .and_then(|f| f.get("path"))
        .and_then(|p| p.as_str())
        .and_then(|p| {
            std::path::Path::new(p)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
        })
        .map(|n| n.strip_suffix(".part").unwrap_or(&n).to_string())
        .unwrap_or_else(|| "-".into())
}

fn print_task_table(tasks: &[Value]) {
    println!(
        "{:<18} {:<10} {:<30} {:>9} {:>12} {:>10}",
        "GID", "Status", "Name", "Progress", "Speed", "Size"
    );
    println!("{}", "-".repeat(93));

    for task in tasks {
        let gid = task.get("gid").and_then(|v| v.as_str()).unwrap_or("-");
        let status = task.get("status").and_then(|v| v.as_str()).unwrap_or("-");
        let total = parse_num(task, "totalLength");
        let completed = parse_num(task, "completedLength");
        let speed = parse_num(task, "downloadSpeed");
        let name = extract_name(task);

        let pct = if total > 0 {
            format!("{:.1}%", completed as f64 / total as f64 * 100.0)
        } else {
            "0.0%".into()
        };

        let display_name = if name.chars().count() > 28 {
            let truncated: String = name.chars().take(25).collect();
            format!("{}...", truncated)
        } else {
            name
        };

        let speed_str = if speed > 0 {
            format_size_speed(speed)
        } else {
            "-".into()
        };

        println!(
            "{:<18} {:<10} {:<30} {:>9} {:>12} {:>10}",
            gid,
            status,
            display_name,
            pct,
            speed_str,
            format_size(total),
        );
    }
}

fn print_task_detail(task: &Value) {
    let gid = task.get("gid").and_then(|v| v.as_str()).unwrap_or("-");
    let status = task.get("status").and_then(|v| v.as_str()).unwrap_or("-");
    let total = parse_num(task, "totalLength");
    let completed = parse_num(task, "completedLength");
    let dl_speed = parse_num(task, "downloadSpeed");
    let ul_speed = parse_num(task, "uploadSpeed");
    let name = extract_name(task);

    let pct = if total > 0 {
        format!("{:.1}%", completed as f64 / total as f64 * 100.0)
    } else {
        "0.0%".into()
    };

    println!("GID:       {}", gid);
    println!("Name:      {}", name);
    println!("Status:    {}", status);
    println!("Size:      {}", format_size(total));
    println!("Completed: {} ({})", format_size(completed), pct);
    println!("DL Speed:  {}", format_size_speed(dl_speed));
    println!("UL Speed:  {}", format_size_speed(ul_speed));

    if let Some(err) = task.get("errorMessage").and_then(|v| v.as_str()) {
        if !err.is_empty() {
            println!("Error:     {}", err);
        }
    }
}
