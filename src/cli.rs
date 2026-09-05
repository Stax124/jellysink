use crate::config::{
    Config, Credentials, Field, MpvArgs, Paths, device_name, normalize_server_url,
};
use crate::instance::{self, InstanceLock};
use crate::jellyfin::auth::login;
use crate::signal::Signal;
use crate::tray;
use crate::usage_err;
use crate::{APP_NAME, VERSION};
use color_eyre::eyre::WrapErr;
use dialoguer::{Input, Password, theme::ColorfulTheme};
use std::ffi::OsStr;
use uuid::Uuid;

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

pub async fn cmd_login(paths: &Paths) -> color_eyre::Result<()> {
    paths.ensure()?;
    let theme = ColorfulTheme::default();

    let server = Input::<String>::with_theme(&theme)
        .with_prompt("Server URL")
        .default("http://localhost:8096".to_string())
        .validate_with(|input: &String| normalize_server_url(input).map(|_| ()))
        .interact_text()
        .wrap_err("reading server URL")?;
    let server = normalize_server_url(&server)?;

    let username = Input::<String>::with_theme(&theme)
        .with_prompt("Username")
        .validate_with(|input: &String| {
            if input.trim().is_empty() {
                Err("username cannot be empty")
            } else {
                Ok(())
            }
        })
        .interact_text()
        .wrap_err("reading username")?;

    let password = Password::with_theme(&theme)
        .with_prompt("Password")
        .allow_empty_password(true)
        .interact()
        .wrap_err("reading password")?;

    let existing = Credentials::load(paths)?;
    let device_id = existing
        .map(|c| c.device_id)
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    let creds = login(&server, &username, &password, &device_id).await?;
    creds.save(paths)?;
    println!("Logged in as {} on {}", creds.username, creds.server);
    Ok(())
}

pub fn cmd_logout(paths: &Paths) -> color_eyre::Result<()> {
    Credentials::remove(paths)?;
    println!("Logged out.");
    Ok(())
}

pub fn cmd_config_path(paths: &Paths) -> color_eyre::Result<()> {
    println!("{}", paths.config_dir.display());
    Ok(())
}

pub fn cmd_config_get(paths: &Paths, key: Option<&str>) -> color_eyre::Result<()> {
    let Some(key) = key else {
        // The whole configuration, including mpv_args — which lives in its own
        // file and used to be silently omitted from this dump.
        let cfg = Config::load(paths)?;
        print!("{}", cfg.to_toml()?);
        let args = MpvArgs::get(paths)?;
        if !args.trim().is_empty() {
            println!("\n# {}", Field::MpvArgs.name());
            println!("{}", args.trim_end());
        }
        return Ok(());
    };
    let field = Field::parse(key)?;
    let out = match Config::load(paths)?.get(field) {
        Some(v) => v,
        // Not in config.toml; a running daemon re-reads it on every mpv spawn.
        None => MpvArgs::get(paths)?,
    };
    println!("{}", out.trim_end());
    Ok(())
}

pub fn cmd_config_set(paths: &Paths, key: &str, value: &str) -> color_eyre::Result<()> {
    let field = Field::parse(key)?;
    let mut cfg = Config::load(paths)?;
    if cfg.set(field, value)? {
        cfg.save(paths)?;
    } else {
        MpvArgs::save(paths, value)?;
    }
    Ok(())
}

pub fn cmd_stop(paths: &Paths) -> color_eyre::Result<()> {
    instance::request_stop(paths)
}

pub async fn cmd_run(paths: Paths) -> color_eyre::Result<()> {
    tracing::info!("jellysink {VERSION}");

    let config = Config::load_or_create(&paths)?;
    let creds = Credentials::load(&paths)?
        .ok_or_else(|| usage_err("not logged in; run `jellysink login` first"))?;

    let exe = crate::update::restart_exe_path(
        &std::env::current_exe().wrap_err("resolving current executable")?,
    );

    let _lock = InstanceLock::acquire(&paths)?;
    tracing::info!(
        server = %creds.server,
        user = %creds.username,
        device = %device_name(),
        autoplay = config.autoplay,
        "starting"
    );

    let shutdown = Signal::new();
    let restart = Signal::new();
    let tray = tray::start(shutdown.clone()).await;
    spawn_update_check(tray.as_ref().map(|t| t.handle.clone()));
    if let Some(apply) = tray.as_ref().map(|t| t.apply.clone()) {
        let update_paths = paths.clone();
        let apply_exe = exe.clone();
        let apply_restart = restart.clone();
        tokio::spawn(async move {
            loop {
                apply.fired().await;
                apply.take();
                apply_update_from_daemon(
                    update_paths.clone(),
                    apply_exe.clone(),
                    apply_restart.clone(),
                )
                .await;
            }
        });
    }

    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .wrap_err("SIGTERM handler")?;
    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .wrap_err("SIGINT handler")?;

    let stop_paths = paths.clone();
    let stop_shutdown = shutdown.clone();
    let stop_restart = restart.clone();
    let stop_fut =
        async move { instance::listen_stop(&stop_paths, stop_shutdown, stop_restart).await };

    let session_shutdown = shutdown.clone();
    let session_fut = crate::runtime::run(config, creds, paths, session_shutdown);
    tokio::pin!(session_fut, stop_fut);

    let mut do_restart = false;
    let outcome = tokio::select! {
        r = &mut session_fut => r,
        r = &mut stop_fut => r,
        _ = sigterm.recv() => {
            tracing::info!("SIGTERM");
            Ok(())
        }
        _ = sigint.recv() => {
            tracing::info!("SIGINT");
            Ok(())
        }
        _ = restart.fired() => {
            tracing::info!("restart requested");
            do_restart = true;
            shutdown.fire();
            session_fut.await
        }
    };
    shutdown.fire();
    outcome?;
    if do_restart {
        tracing::info!(path = %exe.display(), "replacing process with updated binary");
        let err = crate::update::exec_updated(&exe);
        tracing::error!("restart after update failed: {err}");
        return Err(err).wrap_err("restarting after update");
    }
    Ok(())
}

pub async fn cmd_update(
    paths: &Paths,
    check_only: bool,
    from_tray: bool,
) -> color_eyre::Result<()> {
    if check_only {
        match crate::update::check().await? {
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
    let Some(offer) = crate::update::check().await? else {
        println!("{APP_NAME} {VERSION} is up to date.");
        return Ok(());
    };
    println!(
        "Downloading {APP_NAME} v{} (running {VERSION})...",
        offer.version
    );
    let status = crate::update::install(true).await?;
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

fn spawn_update_check(handle: Option<ksni::Handle<tray::CastTray>>) {
    tokio::spawn(async move {
        match crate::update::check().await {
            Ok(Some(offer)) => {
                tracing::info!(version = %offer.version, "update available");
                if let Some(handle) = handle {
                    handle.update(|t| t.set_pending(offer.version)).await;
                }
            }
            Ok(None) => tracing::debug!("already up to date"),
            Err(e) => tracing::warn!("update check failed: {e:#}"),
        }
    });
}

async fn spawn_tray_update(paths: &Paths, exe: &std::path::Path) -> std::io::Result<()> {
    crate::terminal::spawn_in_terminal(&[
        exe.as_os_str(),
        OsStr::new("--config"),
        paths.config_dir.as_os_str(),
        OsStr::new("update"),
        OsStr::new("--from-tray"),
    ])
    .await
}

async fn apply_update_from_daemon(paths: Paths, exe: std::path::PathBuf, restart: Signal) {
    if let Err(e) = spawn_tray_update(&paths, &exe).await {
        tracing::warn!("could not open a terminal for the update ({e}); updating silently");
        match crate::update::install(false).await {
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
mod tests {
    use super::*;

    #[test]
    fn after_install_restarts_from_tray_when_running() {
        assert_eq!(after_install(true, true, true), AfterInstall::Restart);
    }

    #[test]
    fn after_install_stops_from_cli_when_running() {
        assert_eq!(after_install(false, true, true), AfterInstall::Stop);
    }

    #[test]
    fn after_install_does_nothing_when_not_running_or_not_updated() {
        assert_eq!(after_install(true, false, true), AfterInstall::None);
        assert_eq!(after_install(false, true, false), AfterInstall::None);
        assert_eq!(after_install(true, true, false), AfterInstall::None);
    }
}
