//! jellytui: a terminal frontend that browses Jellyfin and casts to a running
//! jellysink. Never calls `init_tracing`, which would paint over the alternate
//! screen; see `crate::logs` and `specs/tui.md`.

mod app;
mod cli;
mod cover;
#[cfg(test)]
mod test_support;

mod keys;
mod logs;
mod nav;

mod view;

use clap::{Parser, Subcommand};
use color_eyre::eyre::Result;
use jellysink_core::UsageError;
use jellysink_core::config::{Config, Credentials, Paths};
use jellysink_core::jellyfin::auth::Api;
use jellysink_core::usage_err;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "jellytui",
    version = env!("CARGO_PKG_VERSION"),
    about = "Browse Jellyfin in the terminal and play in jellysink"
)]
struct Cli {
    /// Configuration directory (default: ~/.config/jellysink)
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Download and install the latest GitHub release
    Update {
        /// Only check; do not download
        #[arg(long)]
        check: bool,
    },
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let paths = Paths::from_override(cli.config)?;
    jellysink_core::install_crypto_provider();

    let result = match cli.command {
        Some(Command::Update { check }) => cli::cmd_update(&paths, check).await,
        None => run(paths).await,
    };
    match result {
        Ok(()) => Ok(()),
        Err(err) => {
            if let Some(usage) = err.downcast_ref::<UsageError>() {
                eprintln!("{usage}");
                std::process::exit(1);
            }
            Err(err)
        }
    }
}

async fn run(paths: Paths) -> Result<()> {
    // Before anything worth logging happens, and while a bad filter can still
    // be reported on the normal screen.
    let logs = logs::install();
    let config = Config::load(&paths)?;
    let credentials = Credentials::load(&paths)?
        .ok_or_else(|| usage_err("not logged in; run `jellysink login` first"))?;
    let api = Api::from_credentials(&credentials)?;
    // Before the alternate screen is taken: the protocol query writes to
    // stdout and reads the terminal's answer back off stdin.
    let picker = cover::detect_picker();
    let disk = cover::CoverDisk::new(paths.cover_cache_dir(), config.cover_cache_mb);
    tokio::task::spawn_blocking({
        let disk = disk.clone();
        move || disk.prune()
    });
    match app::App::new(api, paths.clone(), picker, disk, logs)
        .run()
        .await?
    {
        app::Exit::Quit => Ok(()),
        app::Exit::Update => cli::update_and_restart(&paths).await,
    }
}
