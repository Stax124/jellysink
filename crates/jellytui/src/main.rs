//! jellytui: a terminal frontend that browses Jellyfin and casts to a running
//! jellysink. See `specs/tui.md`.
//!
//! It never calls `init_tracing`: that writes to stdout and would paint over
//! the alternate screen.

mod app;
mod cover;

mod keys;
mod nav;

mod view;

use clap::Parser;
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
    #[arg(long)]
    config: Option<PathBuf>,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let paths = Paths::from_override(cli.config)?;
    jellysink_core::install_crypto_provider();

    match run(paths).await {
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
    let credentials = Credentials::load(&paths)?
        .ok_or_else(|| usage_err("not logged in; run `jellysink login` first"))?;
    let api = Api::from_credentials(&credentials)?;
    // Before the alternate screen is taken: the protocol query writes to
    // stdout and reads the terminal's answer back off stdin.
    let picker = cover::detect_picker();
    let image_scale = Config::load(&paths)?.image_scale;
    app::App::new(api, paths, picker, image_scale).run().await
}
