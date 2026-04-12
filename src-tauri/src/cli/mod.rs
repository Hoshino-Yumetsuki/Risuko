pub mod commands;
pub mod headless;
pub mod progress;
pub mod rpc_client;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "motrix", about = "A full-featured download manager", version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Internal flag set by autostart (hidden from help)
    #[arg(long = "opened-at-login", hide = true)]
    pub opened_at_login: Option<String>,
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

    /// RPC port to connect to (default: 16800)
    #[arg(long, default_value_t = 16800)]
    pub rpc_port: u16,

    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

#[derive(clap::Args)]
pub struct StatusArgs {
    /// Show a specific task by GID
    #[arg(long)]
    pub gid: Option<String>,

    /// RPC port to connect to (default: 16800)
    #[arg(long, default_value_t = 16800)]
    pub rpc_port: u16,

    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

#[derive(clap::Args)]
pub struct PauseArgs {
    /// Task GID to pause
    pub gid: String,

    /// RPC port to connect to (default: 16800)
    #[arg(long, default_value_t = 16800)]
    pub rpc_port: u16,
}

#[derive(clap::Args)]
pub struct ResumeArgs {
    /// Task GID to resume
    pub gid: String,

    /// RPC port to connect to (default: 16800)
    #[arg(long, default_value_t = 16800)]
    pub rpc_port: u16,
}

#[derive(clap::Args)]
pub struct RemoveArgs {
    /// Task GID to remove
    pub gid: String,

    /// RPC port to connect to (default: 16800)
    #[arg(long, default_value_t = 16800)]
    pub rpc_port: u16,
}

pub async fn run(command: Command) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        Command::Download(args) => commands::download(args).await,
        Command::Status(args) => commands::status(args).await,
        Command::Pause(args) => commands::pause(args).await,
        Command::Resume(args) => commands::resume(args).await,
        Command::Remove(args) => commands::remove(args).await,
    }
}
