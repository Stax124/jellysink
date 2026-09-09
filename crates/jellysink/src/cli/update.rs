use crate::daemon::signal::Signal;
use crate::daemon::terminal::spawn_in_terminal;
use crate::daemon::update::{check, install};
use jellysink_core::config::Paths;
use jellysink_core::instance;
use jellysink_core::{APP_NAME, VERSION};
use std::ffi::OsStr;

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
    from_tray: bool,
) -> color_eyre::Result<()> {
    if check_only {
        match check().await? {
            Some(offer) => {
                println!("update available: {} (running {VERSION})", offer.version);
            }
            None => println!("{APP_NAME} {VERSION} is up to date"),
        }
        return Ok(());
    }

    let result = install_and_handoff(paths, from_tray).await;
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

async fn install_and_handoff(paths: &Paths, from_tray: bool) -> color_eyre::Result<()> {
    println!("Checking for updates...");
    let Some(offer) = check().await? else {
        println!("{APP_NAME} {VERSION} is up to date.");
        return Ok(());
    };
    println!(
        "Downloading {APP_NAME} v{} (running {VERSION})...",
        offer.version
    );
    let status = install(true).await?;
    let updated = status.is_updated();
    if updated {
        println!("Updated to version {}.", status.version());
    } else {
        println!("Already up to date.");
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
        match install(false).await {
            Ok(status) if status.is_updated() => {
                tracing::info!(version = %status.version(), "updated; restarting");
                restart.fire();
            }
            Ok(_) => tracing::info!("already up to date"),
            Err(e) => tracing::error!("installing update failed: {e:#}"),
        }
    }
}

#[cfg(test)]
#[path = "update_test.rs"]
mod tests;
