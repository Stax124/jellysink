//! The daemon side of `stop.sock`: the listener that answers the commands
//! `jellysink_core::instance` sends.

use color_eyre::eyre::WrapErr;
use jellysink_core::instance::{InstanceCommand, parse_instance_command};
use jellysink_core::status::PlayerStatus;
use std::fs::Permissions;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::watch;

use crate::daemon::signal::Signal;

/// A client that never writes holds up the next connection until this runs out; it is
/// under the client's reply timeout, so that next client is still answered.
const COMMAND_READ_TIMEOUT: Duration = Duration::from_millis(200);

/// Separate from `listen_stop` so the path exists before the task that answers
/// on it is first polled, and a client cannot connect into an empty backlog.
pub(crate) fn bind_stop_socket(sock: &Path) -> color_eyre::Result<UnixListener> {
    remove_stop_socket(sock);
    let listener =
        UnixListener::bind(sock).wrap_err_with(|| format!("binding {}", sock.display()))?;
    // A bound socket cannot be given a mode up front; the 0700 directory from
    // `Paths::ensure` is what actually keeps it private.
    if let Err(e) = std::fs::set_permissions(sock, Permissions::from_mode(0o600)) {
        tracing::warn!("setting mode 0600 on {}: {e}", sock.display());
    }
    Ok(listener)
}

pub(crate) fn remove_stop_socket(sock: &Path) {
    if let Err(e) = std::fs::remove_file(sock)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!("leaving {} behind: {e}", sock.display());
    }
}

pub(crate) async fn listen_stop(
    listener: UnixListener,
    sock: PathBuf,
    shutdown: Signal,
    restart: Signal,
    status_rx: watch::Receiver<PlayerStatus>,
) {
    loop {
        let mut stream = tokio::select! {
            _ = shutdown.fired() => break,
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => stream,
                Err(e) => {
                    tracing::debug!("accepting on {}: {e}", sock.display());
                    continue;
                }
            },
        };
        match read_command(&mut stream).await {
            Some(InstanceCommand::Stop) => {
                tracing::info!("stop requested");
                shutdown.fire();
                break;
            }
            Some(InstanceCommand::Restart) => {
                tracing::info!("restart requested");
                restart.fire();
            }
            Some(InstanceCommand::Status) => write_status(&mut stream, &status_rx).await,
            None => {}
        }
    }
    // The path goes with the loop: nothing answers on it from here, and a client
    // that finds it gone reports the daemon as not running instead of stalling.
    remove_stop_socket(&sock);
}

async fn read_command(stream: &mut UnixStream) -> Option<InstanceCommand> {
    let mut buf = [0u8; 64];
    let Ok(read) = tokio::time::timeout(COMMAND_READ_TIMEOUT, stream.read(&mut buf)).await else {
        tracing::debug!("dropping a stop.sock client that sent nothing");
        return None;
    };
    parse_instance_command(std::str::from_utf8(&buf[..read.ok()?]).ok()?)
}

async fn write_status(stream: &mut UnixStream, status_rx: &watch::Receiver<PlayerStatus>) {
    let status = status_rx.borrow().clone();
    match serde_json::to_vec(&status) {
        Ok(payload) => {
            if let Err(e) = stream.write_all(&payload).await {
                tracing::warn!("writing status to stop.sock: {e}");
            }
        }
        Err(e) => tracing::warn!("serializing status: {e}"),
    }
}

#[cfg(test)]
#[path = "instance_test.rs"]
mod tests;
