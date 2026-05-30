use serde_json::{json, Value};

use super::headless;
use super::progress::{self, format_size, format_size_speed};
use super::rpc_client::RpcClient;
use super::{DownloadArgs, PauseArgs, RemoveArgs, ResumeArgs, ServeArgs, StatusArgs};
use risuko_engine::engine::youtube::is_youtube_uri;

fn resolve_rpc_secret(explicit: Option<String>) -> Option<String> {
    explicit
        .filter(|s| !s.is_empty())
        .or_else(read_secret_from_config)
}

/// Read rpc-secret from the config files, returning None if empty or absent.
/// user.json takes precedence over system.json
fn read_secret_from_config() -> Option<String> {
    let config_dir = dirs::config_dir().map(|d| d.join("dev.risuko.app"))?;
    let mut merged = load_config_file(&config_dir.join("system.json"));
    let user = load_config_file(&config_dir.join("user.json"));
    merged.extend(user);
    let secret = merged
        .get("rpc-secret")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if secret.is_empty() {
        None
    } else {
        Some(secret)
    }
}

fn load_config_file(path: &std::path::Path) -> serde_json::Map<String, Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|data| serde_json::from_str::<Value>(&data).ok())
        .and_then(|v| {
            if let Value::Object(m) = v {
                Some(m)
            } else {
                None
            }
        })
        .unwrap_or_default()
}

pub async fn download(args: DownloadArgs) -> Result<(), Box<dyn std::error::Error>> {
    let mut secret = resolve_rpc_secret(args.rpc_secret.clone());
    let client = RpcClient::new(args.rpc_port, secret.clone());
    let mut headless_engine = None;

    if !client.is_engine_running().await {
        eprintln!("No running Risuko instance found. Starting headless engine...");
        let engine = headless::start_headless_engine(args.rpc_port).await?;
        if secret.is_none() {
            secret = engine.rpc_secret().map(|s| s.to_string());
        }
        headless_engine = Some(engine);
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }

    let client = RpcClient::new(args.rpc_port, secret);
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
    // DoH is a process-wide thing via the engine's global resolver, not a
    // per-task option, so push it through changeGlobalOption before we add the
    // task instead of stuffing it into the per-task options map
    let mut doh_global = serde_json::Map::new();
    if let Some(ref doh_url) = args.doh_url {
        doh_global.insert("doh-enable".into(), json!(true));
        doh_global.insert("doh-url".into(), json!(doh_url));
    }
    if let Some(ref doh_bootstrap) = args.doh_bootstrap {
        doh_global.insert("doh-bootstrap".into(), json!(doh_bootstrap));
    }
    if !doh_global.is_empty() {
        client
            .call("risuko.changeGlobalOption", vec![json!(doh_global)])
            .await?;
    }
    if let Some(ref referer) = args.referer {
        options.insert("referer".into(), json!(referer));
    }
    if let Some(ref cookie) = args.cookie {
        // Set cookie as a header
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
    if let Some(ref youtube_format) = args.youtube_format {
        options.insert("youtube-format".into(), json!(youtube_format));
    }
    if let Some(ratio) = args.seed_ratio {
        options.insert("seed-ratio".into(), json!(ratio.to_string()));
    }
    if let Some(time) = args.seed_time {
        options.insert("seed-time".into(), json!(time.to_string()));
    }

    // Add custom headers
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

    // Detect torrent file
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
    } else if is_youtube_uri(args.url.trim()) {
        let result = client
            .call("risuko.addYouTube", vec![json!(&args.url), json!(options)])
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

    progress::watch_download(client, &gid, args.json).await
}

pub async fn status(args: StatusArgs) -> Result<(), Box<dyn std::error::Error>> {
    let secret = resolve_rpc_secret(args.rpc_secret);
    let client = RpcClient::new(args.rpc_port, secret);
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

        let mut tasks: Vec<Value> = Vec::new();
        if let Some(arr) = active.as_array() {
            tasks.extend(arr.iter().cloned());
        }

        let page_size = 100;
        for method in ["risuko.tellWaiting", "risuko.tellStopped"] {
            let mut offset = 0;
            loop {
                let page = client
                    .call(
                        method,
                        vec![json!(offset), json!(page_size), all_keys.clone()],
                    )
                    .await?;
                let arr = page.as_array();
                let count = arr.map(|a| a.len()).unwrap_or(0);
                if let Some(items) = arr {
                    tasks.extend(items.iter().cloned());
                }
                if count < page_size {
                    break;
                }
                offset += page_size;
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

pub async fn pause(args: PauseArgs) -> Result<(), Box<dyn std::error::Error>> {
    let secret = resolve_rpc_secret(args.rpc_secret);
    let client = RpcClient::new(args.rpc_port, secret);
    require_engine(&client).await?;

    client.call("risuko.pause", vec![json!(args.gid)]).await?;
    println!("Paused: {}", args.gid);
    Ok(())
}

pub async fn resume(args: ResumeArgs) -> Result<(), Box<dyn std::error::Error>> {
    let secret = resolve_rpc_secret(args.rpc_secret);
    let client = RpcClient::new(args.rpc_port, secret);
    require_engine(&client).await?;

    client.call("risuko.unpause", vec![json!(args.gid)]).await?;
    println!("Resumed: {}", args.gid);
    Ok(())
}

pub async fn remove(args: RemoveArgs) -> Result<(), Box<dyn std::error::Error>> {
    let secret = resolve_rpc_secret(args.rpc_secret);
    let client = RpcClient::new(args.rpc_port, secret);
    require_engine(&client).await?;

    client.call("risuko.remove", vec![json!(args.gid)]).await?;
    println!("Removed: {}", args.gid);
    Ok(())
}

async fn require_engine(client: &RpcClient) -> Result<(), Box<dyn std::error::Error>> {
    if !client.is_engine_running().await {
        return Err("No Risuko instance running. Start the app or run a download first.".into());
    }
    Ok(())
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
        "{:<12} {:<10} {:<30} {:>9} {:>12} {:>10}",
        "GID", "Status", "Name", "Progress", "Speed", "Size"
    );
    println!("{}", "-".repeat(87));

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
            "{:<12} {:<10} {:<30} {:>9} {:>12} {:>10}",
            &gid[..gid.len().min(10)],
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

pub async fn serve(args: ServeArgs) -> Result<(), Box<dyn std::error::Error>> {
    eprintln!("Starting Risuko engine on port {}...", args.rpc_port);
    let engine = headless::start_headless_engine(args.rpc_port).await?;
    eprintln!("Risuko engine running. Press Ctrl+C to stop.");

    tokio::signal::ctrl_c().await?;
    eprintln!("\nShutting down...");
    engine.shutdown().await;
    Ok(())
}
