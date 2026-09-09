//! A terminal Jellyfin frontend that casts to a running jellysink.

use clap::Parser;
use color_eyre::eyre::Result;
use jellysink_core::UsageError;
use jellysink_core::config::Paths;
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

    match jellysink::tui::run(paths).await {
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
