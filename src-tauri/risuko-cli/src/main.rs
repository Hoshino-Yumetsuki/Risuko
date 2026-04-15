mod commands;
mod progress;
mod rpc_client;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "risuko", about = "Risuko download engine CLI", version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Download a file from a URL, magnet link, or torrent file
    Download(DownloadArgs),

    /// Show status of downloads
    Status(StatusArgs),

    /// Pause a download
    Pause(PauseArgs),

    /// Resume a paused download
    Resume(ResumeArgs),

    /// Remove a download
    Remove(RemoveArgs),

    /// Pause all active downloads
    PauseAll(RpcArgs),

    /// Resume all paused downloads
    ResumeAll(RpcArgs),

    /// Show global download/upload stats
    GlobalStat(RpcArgs),

    /// Show files belonging to a download
    Files(GidArgs),

    /// Show peers for a torrent download
    Peers(GidArgs),

    /// Purge completed/error/removed download results
    Purge(RpcArgs),

    /// Manage configuration
    Config(ConfigCommand),

    /// Manage RSS feeds
    Rss(RssCommand),

    /// Start headless engine (RPC server only, no GUI)
    Serve(ServeArgs),

    /// Request engine shutdown via RPC
    Shutdown(RpcArgs),
}

#[derive(clap::Args)]
pub struct DownloadArgs {
    /// URL, magnet link, or path to a .torrent file
    pub url: String,

    /// Number of connections per download (maps to split)
    #[arg(short = 't', long, default_value_t = 16)]
    pub threads: u32,

    /// Download directory
    #[arg(short, long)]
    pub dir: Option<String>,

    /// Output filename
    #[arg(short, long)]
    pub out: Option<String>,

    /// HTTP header (repeatable, e.g. -H "Cookie: foo=bar")
    #[arg(short = 'H', long = "header")]
    pub headers: Vec<String>,

    /// User agent string
    #[arg(long)]
    pub user_agent: Option<String>,

    /// Proxy server URL (e.g. http://proxy:8080)
    #[arg(long)]
    pub proxy: Option<String>,

    /// HTTP referer
    #[arg(long)]
    pub referer: Option<String>,

    /// Cookie string
    #[arg(long)]
    pub cookie: Option<String>,

    /// BT seed ratio (e.g. 1.0)
    #[arg(long)]
    pub seed_ratio: Option<f64>,

    /// BT seed time in minutes
    #[arg(long)]
    pub seed_time: Option<u64>,

    #[arg(long, default_value_t = 16800)]
    pub rpc_port: u16,

    /// RPC secret for authentication
    #[arg(long)]
    pub rpc_secret: Option<String>,

    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

#[derive(clap::Args)]
pub struct StatusArgs {
    /// Show a specific task by GID
    #[arg(long)]
    pub gid: Option<String>,

    #[arg(long, default_value_t = 16800)]
    pub rpc_port: u16,

    /// RPC secret for authentication
    #[arg(long)]
    pub rpc_secret: Option<String>,

    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

#[derive(clap::Args)]
pub struct PauseArgs {
    /// Task GID to pause
    pub gid: String,

    #[arg(long, default_value_t = 16800)]
    pub rpc_port: u16,

    /// RPC secret for authentication
    #[arg(long)]
    pub rpc_secret: Option<String>,
}

#[derive(clap::Args)]
pub struct ResumeArgs {
    /// Task GID to resume
    pub gid: String,

    #[arg(long, default_value_t = 16800)]
    pub rpc_port: u16,

    /// RPC secret for authentication
    #[arg(long)]
    pub rpc_secret: Option<String>,
}

#[derive(clap::Args)]
pub struct RemoveArgs {
    /// Task GID to remove
    pub gid: String,

    #[arg(long, default_value_t = 16800)]
    pub rpc_port: u16,

    /// RPC secret for authentication
    #[arg(long)]
    pub rpc_secret: Option<String>,
}

#[derive(clap::Args)]
pub struct RpcArgs {
    #[arg(long, default_value_t = 16800)]
    pub rpc_port: u16,

    /// RPC secret for authentication
    #[arg(long)]
    pub rpc_secret: Option<String>,

    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

#[derive(clap::Args)]
pub struct GidArgs {
    /// Task GID
    pub gid: String,

    #[arg(long, default_value_t = 16800)]
    pub rpc_port: u16,

    /// RPC secret for authentication
    #[arg(long)]
    pub rpc_secret: Option<String>,

    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

#[derive(clap::Args)]
pub struct ServeArgs {
    #[arg(long, default_value_t = 16800)]
    pub rpc_port: u16,
}

#[derive(clap::Args)]
pub struct ConfigCommand {
    #[command(subcommand)]
    pub action: ConfigAction,
}

#[derive(Subcommand)]
pub enum ConfigAction {
    /// Get a config value
    Get {
        /// Config key
        key: String,
    },
    /// Set a config value
    Set {
        /// Config key
        key: String,
        /// Config value (JSON)
        value: String,
    },
    /// List all config values
    List {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(clap::Args)]
pub struct RssCommand {
    #[command(subcommand)]
    pub action: RssAction,
}

#[derive(Subcommand)]
pub enum RssAction {
    /// Add an RSS feed
    Add {
        /// Feed URL
        url: String,
        /// RPC port
        #[arg(long, default_value_t = 16800)]
        rpc_port: u16,
        /// RPC secret for authentication
        #[arg(long)]
        rpc_secret: Option<String>,
    },
    /// List all RSS feeds
    List {
        /// RPC port
        #[arg(long, default_value_t = 16800)]
        rpc_port: u16,
        /// RPC secret for authentication
        #[arg(long)]
        rpc_secret: Option<String>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Refresh all RSS feeds
    Refresh {
        /// RPC port
        #[arg(long, default_value_t = 16800)]
        rpc_port: u16,
        /// RPC secret for authentication
        #[arg(long)]
        rpc_secret: Option<String>,
    },
    /// Remove an RSS feed
    Remove {
        /// Feed ID
        id: String,
        /// RPC port
        #[arg(long, default_value_t = 16800)]
        rpc_port: u16,
        /// RPC secret for authentication
        #[arg(long)]
        rpc_secret: Option<String>,
    },
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let cli = Cli::parse();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to build tokio runtime");

    let code = rt.block_on(async {
        match run(cli.command).await {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("Error: {}", e);
                1
            }
        }
    });

    std::process::exit(code);
}

async fn run(command: Command) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        Command::Download(args) => commands::download(args).await,
        Command::Status(args) => commands::status(args).await,
        Command::Pause(args) => commands::pause(args).await,
        Command::Resume(args) => commands::resume(args).await,
        Command::Remove(args) => commands::remove(args).await,
        Command::PauseAll(args) => commands::pause_all(args).await,
        Command::ResumeAll(args) => commands::resume_all(args).await,
        Command::GlobalStat(args) => commands::global_stat(args).await,
        Command::Files(args) => commands::files(args).await,
        Command::Peers(args) => commands::peers(args).await,
        Command::Purge(args) => commands::purge(args).await,
        Command::Config(cmd) => commands::config(cmd).await,
        Command::Rss(cmd) => commands::rss(cmd).await,
        Command::Serve(args) => commands::serve(args).await,
        Command::Shutdown(args) => commands::shutdown(args).await,
    }
}
