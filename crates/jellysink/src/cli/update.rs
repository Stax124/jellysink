use crate::daemon::signal::Signal;
use crate::daemon::terminal::spawn_in_terminal;
use jellysink_core::config::Paths;
use jellysink_core::instance;
use jellysink_core::update::{JELLYTUI_BIN, check, install, sibling_binary};
use jellysink_core::{APP_NAME, VERSION};
use std::ffi::OsStr;

const BIN_NAME: &str = env!("CARGO_BIN_NAME");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AfterInstall {
    None,
    Stop,
    Restart,
}

fn after_install(from_tray: bool, daemon_running: bool, updated: bool) -> AfterInstall {
    if !updated || !daemon_running {
        AfterInstall::None
    } else if from_tray {
        AfterInstall::Restart
    } else {
        AfterInstall::Stop
    }
}

pub(crate) async fn cmd_update(
    paths: &Paths,
    check_only: bool,
    force: bool,
    from_tray: bool,
) -> color_eyre::Result<()> {
    if check_only {
        match check(BIN_NAME).await? {
            Some(version) => println!("update available: {version} (running {VERSION})"),
            None => println!("{APP_NAME} {VERSION} is up to date"),
        }
        return Ok(());
    }

    let result = install_and_handoff(paths, force, from_tray).await;
    if from_tray {
        if let Err(e) = &result {
            eprintln!("{e:#}");
        }
        println!();
        println!("Press Enter to close");
        let _ = std::io::stdin().read_line(&mut String::new());
        if result.is_err() {
            std::process::exit(1);
        }
        return Ok(());
    }
    result
}

async fn install_and_handoff(
    paths: &Paths,
    force: bool,
    from_tray: bool,
) -> color_eyre::Result<()> {
    println!("Checking for updates...");
    let mut updated = false;
    let offer = check(BIN_NAME).await?;
    if offer.is_none() && !force {
        println!("{APP_NAME} {VERSION} is up to date.");
    } else {
        match &offer {
            Some(offer) => println!("Downloading {APP_NAME} v{offer} (running {VERSION})..."),
            None => println!("Reinstalling {APP_NAME} {VERSION}..."),
        }
        match install(BIN_NAME, None, true, force).await? {
            Some(version) => {
                println!("Updated to version {version}.");
                updated = true;
            }
            None => println!("Already up to date."),
        }
    }
    // Unconditional: the frontend updates separately, so it can be behind a
    // daemon that is already current.
    if let Some(path) = sibling_binary(JELLYTUI_BIN) {
        match install(JELLYTUI_BIN, Some(&path), true, force).await {
            Ok(Some(version)) => println!("Updated {JELLYTUI_BIN} to version {version}."),
            Ok(None) => println!("{JELLYTUI_BIN} is up to date."),
            // Our own update has landed by now, so this is reported not raised.
            Err(e) => eprintln!("could not update {JELLYTUI_BIN}: {e:#}"),
        }
    }
    match after_install(from_tray, instance::is_running(paths), updated) {
        AfterInstall::Restart => match instance::request_restart(paths) {
            Ok(()) => println!("Restarting the running daemon."),
            Err(e) => println!("Updated, but could not restart the daemon: {e:#}"),
        },
        AfterInstall::Stop => {
            instance::request_stop(paths)?;
            println!(
                "The running daemon was stopped. Start jellysink again to use the new version:"
            );
            println!("  systemctl --user start jellysink");
            println!("  {APP_NAME} run");
        }
        AfterInstall::None => {}
    }
    Ok(())
}

async fn spawn_tray_update(paths: &Paths, exe: &std::path::Path) -> std::io::Result<()> {
    spawn_in_terminal(&[
        exe.as_os_str(),
        OsStr::new("--config"),
        paths.config_dir.as_os_str(),
        OsStr::new("update"),
        OsStr::new("--from-tray"),
    ])
    .await
}

pub(super) async fn apply_update_from_daemon(
    paths: Paths,
    exe: std::path::PathBuf,
    restart: Signal,
) {
    if let Err(e) = spawn_tray_update(&paths, &exe).await {
        tracing::warn!("could not open a terminal for the update ({e}); updating silently");
        let installed = install(BIN_NAME, None, false, false).await;
        if let Some(path) = sibling_binary(JELLYTUI_BIN)
            && let Err(e) = install(JELLYTUI_BIN, Some(&path), false, false).await
        {
            tracing::warn!("could not update {JELLYTUI_BIN}: {e:#}");
        }
        match installed {
            Ok(Some(version)) => {
                tracing::info!(%version, "updated; restarting");
                restart.fire();
            }
            Ok(None) => tracing::info!("already up to date"),
            Err(e) => tracing::error!("installing update failed: {e:#}"),
        }
    }
}

#[cfg(test)]
#[path = "update_test.rs"]
mod tests;
