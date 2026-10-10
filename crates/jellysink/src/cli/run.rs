use crate::cli::update::{BIN_NAME, apply_update_from_daemon};
use crate::daemon::control::{
    Control, EVENT_CAPACITY, bind_control_socket, listen_control, remove_control_socket,
};
use crate::daemon::signal::Signal;
use crate::daemon::{mpris, tray};
use color_eyre::eyre::WrapErr;
use jellysink_core::VERSION;
use jellysink_core::config::{Config, Credentials, Paths, device_name};
use jellysink_core::instance::InstanceLock;
use jellysink_core::update::{check, exec_updated, restart_exe_path};

pub(crate) async fn cmd_run(paths: Paths) -> color_eyre::Result<()> {
    tracing::info!("jellysink {VERSION}");

    let config = Config::load_or_create(&paths)?;
    let creds = Credentials::load_required(&paths)?;

    let exe = restart_exe_path(&std::env::current_exe().wrap_err("resolving current executable")?);

    let _lock = InstanceLock::acquire(&paths)?;
    let control_socket = paths.control_socket();
    let control_listener = bind_control_socket(&control_socket)?;

    let shutdown = Signal::new();
    let restart = Signal::new();
    let (status_tx, status_rx) = tokio::sync::watch::channel(
        jellysink_core::status::PlayerStatus::idle(creds.server.clone(), creds.username.clone()),
    );
    let (event_tx, _) = tokio::sync::broadcast::channel(EVENT_CAPACITY);
    let (command_tx, command_rx) = tokio::sync::mpsc::unbounded_channel();
    // Ahead of the tray and mpris, so a client is answered from the first await
    // rather than after however long those take to come up.
    tokio::spawn(listen_control(
        control_listener,
        control_socket.clone(),
        shutdown.clone(),
        Control {
            status_rx: status_rx.clone(),
            event_tx: event_tx.clone(),
            command_tx: command_tx.clone(),
            restart: restart.clone(),
        },
    ));

    tracing::info!(
        server = %creds.server,
        user = %creds.username,
        device = %device_name(),
        autoplay = config.autoplay,
        "starting"
    );

    let tray = tray::start(shutdown.clone()).await;
    spawn_update_check(tray.as_ref().map(|t| t.handle.clone()));
    if let Some(apply) = tray.as_ref().map(|t| t.apply.clone()) {
        let (paths, exe, restart) = (paths.clone(), exe.clone(), restart.clone());
        tokio::spawn(async move {
            loop {
                apply.fired().await;
                apply.take();
                apply_update_from_daemon(&paths, &exe, &restart).await;
            }
        });
    }

    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .wrap_err("SIGTERM handler")?;
    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .wrap_err("SIGINT handler")?;

    if tokio::time::timeout(
        std::time::Duration::from_secs(3),
        mpris::start(status_rx.clone(), command_tx, shutdown.clone()),
    )
    .await
    .is_err()
    {
        tracing::warn!(
            "mpris unavailable (timed out connecting to session bus); media keys and desktop widgets won't see jellysink"
        );
    }

    let session_shutdown = shutdown.clone();
    let session_fut = crate::runtime::run(
        config,
        creds,
        paths,
        session_shutdown,
        status_tx,
        event_tx,
        command_rx,
    );
    tokio::pin!(session_fut);

    let mut do_restart = false;
    let outcome = tokio::select! {
        r = &mut session_fut => r,
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
    // The guaranteed unlink: nothing awaits the listener task, so the process can
    // exit before it observes `shutdown` and removes its own path.
    remove_control_socket(&control_socket);
    outcome?;
    if do_restart {
        tracing::info!(path = %exe.display(), "replacing process with updated binary");
        let err = exec_updated(&exe);
        tracing::error!("restart after update failed: {err}");
        return Err(err).wrap_err("restarting after update");
    }
    Ok(())
}

fn spawn_update_check(handle: Option<ksni::Handle<tray::CastTray>>) {
    tokio::spawn(async move {
        match check(BIN_NAME).await {
            Ok(Some(version)) => {
                tracing::info!(%version, "update available");
                if let Some(handle) = handle {
                    handle.update(|t| t.set_pending(version)).await;
                }
            }
            Ok(None) => tracing::debug!("already up to date"),
            Err(e) => tracing::warn!("update check failed: {e:#}"),
        }
    });
}
