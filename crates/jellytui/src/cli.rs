//! `jellytui update`. Output here is `println!`: it runs in front of a user on
//! the normal screen, never behind the alternate one.

use color_eyre::eyre::{Result, WrapErr};
use jellysink_core::VERSION;
use jellysink_core::config::Paths;
use jellysink_core::instance;
use jellysink_core::update::{
    JELLYSINK_BIN, check, exec_updated, install, restart_exe_path, sibling_binary,
};

const BIN_NAME: &str = env!("CARGO_BIN_NAME");

pub(crate) async fn cmd_update(paths: &Paths, check_only: bool) -> Result<()> {
    if check_only {
        match check(BIN_NAME).await? {
            Some(version) => println!("update available: {version} (running {VERSION})"),
            None => println!("{BIN_NAME} {VERSION} is up to date"),
        }
        return Ok(());
    }

    match check(BIN_NAME).await? {
        Some(offer) => {
            println!("Downloading {BIN_NAME} v{offer} (running {VERSION})...");
            match install(BIN_NAME, None, true).await? {
                Some(version) => println!("Updated to version {version}."),
                None => println!("Already up to date."),
            }
        }
        None => println!("{BIN_NAME} {VERSION} is up to date."),
    }

    // The daemon's binary, but never the daemon: the install renames over the
    // path, so a running jellysink plays on until the user restarts it.
    if let Some(path) = sibling_binary(JELLYSINK_BIN) {
        match install(JELLYSINK_BIN, Some(&path), true).await {
            Ok(Some(version)) => {
                println!("Updated {JELLYSINK_BIN} to version {version}.");
                if instance::is_running(paths) {
                    println!("The running daemon stays on the old one until it restarts:");
                    println!("  systemctl --user restart {JELLYSINK_BIN}");
                }
            }
            Ok(None) => println!("{JELLYSINK_BIN} is up to date."),
            // Our own update has landed by now, so this is reported not raised.
            Err(e) => eprintln!("could not update {JELLYSINK_BIN}: {e:#}"),
        }
    }
    Ok(())
}

/// For the `u` key: the user asked from inside the terminal UI and expects it
/// back, so the new binary takes this process over.
pub(crate) async fn update_and_restart(paths: &Paths) -> Result<()> {
    cmd_update(paths, false).await?;
    println!("Restarting...");
    let exe = restart_exe_path(&std::env::current_exe().wrap_err("resolving current executable")?);
    Err(exec_updated(&exe)).wrap_err_with(|| format!("restarting {}", exe.display()))
}
