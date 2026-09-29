use crate::daemon::signal::Signal;
use crate::daemon::terminal::spawn_in_terminal;
use jellysink_core::config::Paths;
use jellysink_core::update::{JELLYTUI_BIN, install, install_both, print_check, sibling_binary};
use std::ffi::OsStr;
use std::path::Path;

pub(super) const BIN_NAME: &str = env!("CARGO_BIN_NAME");

pub(crate) async fn cmd_update(
    paths: &Paths,
    check_only: bool,
    force: bool,
    from_tray: bool,
) -> color_eyre::Result<()> {
    if check_only {
        return print_check(BIN_NAME).await;
    }

    let result = install_both(paths, BIN_NAME, force).await;
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

pub(super) async fn apply_update_from_daemon(paths: &Paths, exe: &Path, restart: &Signal) {
    let opened = spawn_in_terminal(&[
        exe.as_os_str(),
        OsStr::new("--config"),
        paths.config_dir.as_os_str(),
        OsStr::new("update"),
        OsStr::new("--from-tray"),
    ])
    .await;
    if let Err(e) = opened {
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
