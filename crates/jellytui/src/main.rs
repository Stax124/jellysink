//! jellytui: browses Jellyfin in the terminal and casts to a running jellysink.
//! Never calls `init_tracing`, whose `fmt` layer would paint over the alternate screen.

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
use jellysink_core::config::{Config, Credentials, Paths};
use jellysink_core::jellyfin::auth::Api;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "jellytui",
    version,
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
        /// Reinstall the latest release even when it is already installed
        #[arg(long, conflicts_with = "check")]
        force: bool,
    },
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let paths = Paths::from_override(cli.config)?;
    jellysink_core::install_crypto_provider();

    let result = match cli.command {
        Some(Command::Update { check, force }) => cli::cmd_update(&paths, check, force).await,
        None => run(paths).await,
    };
    jellysink_core::error::exit_on_usage_error(result)
}

async fn run(paths: Paths) -> Result<()> {
    // Before anything worth logging happens, and while a bad filter can still
    // be reported on the normal screen.
    let logs = logs::install();
    let config = Config::load(&paths)?;
    let credentials = Credentials::load_required(&paths)?;
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
