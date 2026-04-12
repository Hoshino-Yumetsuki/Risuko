#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

use clap::Parser;
use motrix_lib::cli;

fn main() {
    let parsed = cli::Cli::parse();

    if let Some(command) = parsed.command {
        // CLI mode: run the command and exit
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to build tokio runtime");

        let code = rt.block_on(async {
            match cli::run(command).await {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("Error: {}", e);
                    1
                }
            }
        });

        std::process::exit(code);
    }

    // GUI mode: launch Tauri app
    motrix_lib::run();
}
