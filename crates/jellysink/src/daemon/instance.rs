//! The daemon side of `stop.sock`: the listener that answers the commands
//! `jellysink_core::instance` sends.

use color_eyre::eyre::WrapErr;
use jellysink_core::config::Paths;
use jellysink_core::instance::{InstanceCommand, parse_instance_command};
use jellysink_core::status::PlayerStatus;
use std::os::unix::fs::PermissionsExt;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixListener;

use crate::daemon::signal::Signal;

pub(crate) async fn listen_stop(
    paths: &Paths,
    shutdown: Signal,
    restart: Signal,
    status_rx: tokio::sync::watch::Receiver<PlayerStatus>,
) -> color_eyre::Result<()> {
    let sock = paths.stop_socket();
    let _ = std::fs::remove_file(&sock);
    let listener =
        UnixListener::bind(&sock).wrap_err_with(|| format!("binding {}", sock.display()))?;
    // A bound socket cannot be given a mode up front; the 0700 directory from
    // `Paths::ensure` is what actually keeps it private.
    if let Ok(meta) = std::fs::metadata(&sock) {
        let mut perms = meta.permissions();
        perms.set_mode(0o600);
        let _ = std::fs::set_permissions(&sock, perms);
    }

    loop {
        tokio::select! {
            _ = shutdown.fired() => break,
            accepted = listener.accept() => {
                match accepted {
                    Ok((mut stream, _)) => {
                        let mut buf = vec![0u8; 64];
                        if let Ok(n) =
                            tokio::io::AsyncReadExt::read(&mut stream, &mut buf).await
                        {
                            match parse_instance_command(
                                std::str::from_utf8(&buf[..n]).unwrap_or(""),
                            ) {
                                Some(InstanceCommand::Stop) => {
                                    tracing::info!("stop requested");
                                    shutdown.fire();
                                    break;
                                }
                                Some(InstanceCommand::Restart) => {
                                    tracing::info!("restart requested");
                                    restart.fire();
                                }
                                Some(InstanceCommand::Status) => {
                                    let status = status_rx.borrow().clone();
                                    if let Ok(payload) = serde_json::to_vec(&status) {
                                        let _ = stream.write_all(&payload).await;
                                    }
                                    let _ = stream.shutdown().await;
                                }
                                None => {}
                            }
                        }
                    }
                    Err(e) => {
                        tracing::debug!("stop socket accept: {e}");
                    }
                }
            }
        }
    }
    let _ = std::fs::remove_file(&sock);
    Ok(())
}

#[cfg(test)]
#[path = "instance_test.rs"]
mod tests;
