use crate::config::{Config, Credentials, MpvArgs, Paths, device_name, normalize_server_url};
use crate::instance::{self, InstanceLock};
use crate::jellyfin::auth::login;
use crate::tray;
use crate::usage_err;
use crate::{APP_NAME, VERSION};
use color_eyre::eyre::WrapErr;
use dialoguer::{Input, Password, theme::ColorfulTheme};
use std::sync::Arc;
use tokio::sync::Notify;
use uuid::Uuid;

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
    // `mpv_args` lives in its own file so a running daemon re-reads it on
    // every mpv spawn; it is not part of config.toml.
    let out = match key {
        Some("mpv_args") => MpvArgs::get(paths)?,
        _ => {
            let cfg = Config::load(paths)?;
            cfg.get(key)?
        }
    };
    print!("{out}");
    if key.is_some() {
        println!();
    }
    Ok(())
}

pub fn cmd_config_set(paths: &Paths, key: &str, value: &str) -> color_eyre::Result<()> {
    if key == "mpv_args" {
        MpvArgs::save(paths, value)?;
        return Ok(());
    }
    let mut cfg = Config::load(paths)?;
    cfg.set(key, value)?;
    cfg.save(paths)?;
    Ok(())
}

pub fn cmd_stop(paths: &Paths) -> color_eyre::Result<()> {
    instance::request_stop(paths)
}

pub async fn cmd_run(paths: Paths) -> color_eyre::Result<()> {
    let config = Config::load(&paths)?;
    let creds = Credentials::load(&paths)?
        .ok_or_else(|| usage_err("not logged in; run `jellysink login` first"))?;

    let _lock = InstanceLock::acquire(&paths)?;
    tracing::info!(
        server = %creds.server,
        user = %creds.username,
        device = %device_name(),
        autoplay = config.autoplay,
        "starting"
    );

    let shutdown = Arc::new(Notify::new());
    let tray = tray::start(shutdown.clone()).await;
    spawn_update_check(tray.as_ref().map(|t| t.handle.clone()));
    if let Some(apply) = tray.as_ref().map(|t| t.apply.clone()) {
        tokio::spawn(async move {
            apply.notified().await;
            apply_update_from_daemon().await;
        });
    }

    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .wrap_err("SIGTERM handler")?;
    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .wrap_err("SIGINT handler")?;

    let stop_paths = paths.clone();
    let stop_shutdown = shutdown.clone();
    let stop_task = async move { instance::listen_stop(&stop_paths, stop_shutdown).await };

    let session_shutdown = shutdown.clone();
    let session_task = crate::runtime::run(config, creds, paths, session_shutdown);

    tokio::select! {
        r = session_task => r?,
        r = stop_task => r?,
        _ = sigterm.recv() => tracing::info!("SIGTERM"),
        _ = sigint.recv() => tracing::info!("SIGINT"),
    }
    shutdown.notify_waiters();
    Ok(())
}

pub async fn cmd_update(paths: &Paths, check_only: bool) -> color_eyre::Result<()> {
    if check_only {
        match crate::update::check().await? {
            Some(offer) => {
                println!("update available: {} (running {VERSION})", offer.version);
            }
            None => println!("{APP_NAME} {VERSION} is up to date"),
        }
        return Ok(());
    }

    let status = crate::update::install(true).await?;
    if status.is_updated() {
        println!("Updated to version {}.", status.version());
        if instance::is_running(paths) {
            instance::request_stop(paths)?;
            println!(
                "The running daemon was stopped. Start jellysink again to use the new version:"
            );
            println!("  systemctl --user start jellysink");
            println!("  {APP_NAME} run");
        }
    } else {
        println!("Already up to date.");
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

async fn apply_update_from_daemon() {
    match crate::update::install(false).await {
        Ok(status) if status.is_updated() => {
            tracing::info!(version = %status.version(), "updated; restarting");
            let Err(e) = self_update::restart::restart();
            tracing::error!("restart after update failed: {e}");
        }
        Ok(_) => tracing::info!("already up to date"),
        Err(e) => tracing::error!("installing update failed: {e:#}"),
    }
}
