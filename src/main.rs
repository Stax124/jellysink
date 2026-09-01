use clap::{Parser, Subcommand};
use color_eyre::eyre::Result;
use jellysink::UsageError;
use jellysink::cli;
use jellysink::config::{Config, Paths};
use jellysink::tracing::init_tracing;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "jellysink",
    version = env!("CARGO_PKG_VERSION"),
    about = "Headless Jellyfin cast target that plays in MPV"
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
    /// Log in to a Jellyfin server (username and password)
    Login,
    /// Forget stored credentials
    Logout,
    /// Get or set configuration
    Config {
        #[command(subcommand)]
        action: ConfigCmd,
    },
    /// Run as a cast target (default)
    Run,
    /// Ask a running instance to quit
    Stop,
    /// Download and install the latest GitHub release
    Update {
        /// Only check; do not download
        #[arg(long)]
        check: bool,
        /// Install from the tray: show progress, restart the daemon, wait for Enter
        #[arg(long, hide = true)]
        from_tray: bool,
    },
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// Print the configuration directory
    Path,
    /// Print one key, or the whole file
    Get { key: Option<String> },
    /// Set a key (mpv_path, mpv_args, log_level, autoplay, prepend_previous)
    Set {
        key: String,
        /// Values may start with `-` (e.g. `mpv_args --fullscreen`).
        /// `mpv_args` is stored in mpv_args.conf and re-read on every mpv
        /// spawn, so changes apply without restarting the daemon.
        #[arg(allow_hyphen_values = true)]
        value: String,
    },
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    match try_main().await {
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

async fn try_main() -> Result<()> {
    let cli = Cli::parse();
    let paths = Paths::from_override(cli.config)?;
    let log_level = Config::load(&paths)
        .map(|c| c.log_level)
        .unwrap_or_else(|_| "info".into());

    let _ = rustls::crypto::ring::default_provider().install_default();
    init_tracing(&log_level)?;

    match cli.command.unwrap_or(Command::Run) {
        Command::Login => cli::cmd_login(&paths).await?,
        Command::Logout => cli::cmd_logout(&paths)?,
        Command::Config { action } => match action {
            ConfigCmd::Path => cli::cmd_config_path(&paths)?,
            ConfigCmd::Get { key } => cli::cmd_config_get(&paths, key.as_deref())?,
            ConfigCmd::Set { key, value } => cli::cmd_config_set(&paths, &key, &value)?,
        },
        Command::Run => cli::cmd_run(paths).await?,
        Command::Stop => cli::cmd_stop(&paths)?,
        Command::Update { check, from_tray } => cli::cmd_update(&paths, check, from_tray).await?,
    }
    Ok(())
}
