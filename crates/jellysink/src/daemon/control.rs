//! The daemon side of `control.sock`: one task per client, each pushing the
//! status and events to it and answering its commands. See `specs/session.md`.

use crate::daemon::signal::Signal;
use crate::runtime::CastCommand;
use color_eyre::eyre::WrapErr;
use jellysink_core::VERSION;
use jellysink_core::control::{
    Command, DaemonFrame, Event, FrameReader, Handshake, Origin, PROTOCOL, Request, write_frame,
};
use jellysink_core::status::PlayerStatus;
use serde::Deserialize;
use serde_json::Value;
use std::fs::Permissions;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::UnixListener;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{broadcast, mpsc, watch};
use tokio::task::JoinSet;

/// A connection that never says hello holds one of the [`MAX_CONNECTIONS`]
/// slots until this runs out.
const HELLO_TIMEOUT: Duration = Duration::from_secs(2);
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);
/// Bounds the tasks a runaway local script can make the daemon hold.
const MAX_CONNECTIONS: usize = 16;
pub(crate) const EVENT_CAPACITY: usize = 64;

#[derive(Clone)]
pub(crate) struct Control {
    pub(crate) status_rx: watch::Receiver<PlayerStatus>,
    pub(crate) event_tx: broadcast::Sender<Event>,
    pub(crate) command_tx: mpsc::UnboundedSender<CastCommand>,
    pub(crate) restart: Signal,
}

/// Separate from `listen_control` so the path exists before the task that
/// answers on it is first polled, and a client cannot connect into an empty backlog.
pub(crate) fn bind_control_socket(sock: &Path) -> color_eyre::Result<UnixListener> {
    remove_control_socket(sock);
    let listener =
        UnixListener::bind(sock).wrap_err_with(|| format!("binding {}", sock.display()))?;
    // A bound socket cannot be given a mode up front; the 0700 directory from
    // `Paths::ensure` is what actually keeps it private.
    if let Err(e) = std::fs::set_permissions(sock, Permissions::from_mode(0o600)) {
        tracing::warn!("setting mode 0600 on {}: {e}", sock.display());
    }
    Ok(listener)
}

pub(crate) fn remove_control_socket(sock: &Path) {
    if let Err(e) = std::fs::remove_file(sock)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!("leaving {} behind: {e}", sock.display());
    }
}

pub(crate) async fn listen_control(
    listener: UnixListener,
    sock: PathBuf,
    shutdown: Signal,
    control: Control,
) {
    // Dropping it on the way out aborts every connection still open.
    let mut connections = JoinSet::new();
    let mut client_id = 0;
    loop {
        let stream = tokio::select! {
            _ = shutdown.fired() => break,
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => stream,
                Err(e) => {
                    tracing::debug!("accepting on {}: {e}", sock.display());
                    continue;
                }
            },
        };
        while let Some(joined) = connections.try_join_next() {
            if let Err(e) = joined {
                tracing::warn!("a control connection failed: {e}");
            }
        }
        if connections.len() >= MAX_CONNECTIONS {
            tracing::warn!("turning a control client away: {MAX_CONNECTIONS} already connected");
            continue;
        }
        client_id += 1;
        connections.spawn(serve(stream, client_id, control.clone()));
    }
    // The path goes with the loop: nothing answers on it from here, and a client
    // that finds it gone reports the daemon as not running instead of stalling.
    remove_control_socket(&sock);
}

async fn serve<S>(stream: S, client_id: u64, control: Control)
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    let (read, mut write) = tokio::io::split(stream);
    let mut lines = FrameReader::new(read);
    if !greet(&mut lines, &mut write, client_id).await {
        return;
    }
    let mut status_rx = control.status_rx.clone();
    // Before the snapshot, so nothing falls between the two.
    let mut events = control.event_tx.subscribe();
    let welcome = DaemonFrame::Welcome {
        protocol: PROTOCOL,
        daemon: VERSION.to_string(),
        client_id,
    };
    let snapshot = DaemonFrame::Status {
        status: Box::new(status_rx.borrow_and_update().clone()),
    };
    if !send(&mut write, client_id, &welcome).await || !send(&mut write, client_id, &snapshot).await
    {
        return;
    }
    loop {
        let frame = tokio::select! {
            // A status before an event: a client reading an outcome already has
            // the state it led to.
            biased;
            changed = status_rx.changed() => {
                if changed.is_err() {
                    return;
                }
                DaemonFrame::Status { status: Box::new(status_rx.borrow_and_update().clone()) }
            }
            event = events.recv() => match event {
                Ok(event) => DaemonFrame::Event(event),
                Err(RecvError::Lagged(missed)) => {
                    tracing::warn!(client_id, missed, "dropping a control client that fell behind on events");
                    return;
                }
                Err(RecvError::Closed) => return,
            },
            line = lines.next_line() => match line {
                Ok(Some(line)) => match parse_request(&line) {
                    Err(frame) => frame,
                    Ok(Request { id, command: Command::Restart }) => {
                        // Replied to first, so the updater hears it landed before
                        // the process execs.
                        if send(&mut write, client_id, &DaemonFrame::Reply { id, error: None }).await {
                            tracing::info!("restart requested");
                            control.restart.fire();
                        }
                        continue;
                    }
                    Ok(Request { id, .. }) if !control.status_rx.borrow().server_connected => {
                        DaemonFrame::Reply {
                            id,
                            error: Some("jellysink has no connection to the server".into()),
                        }
                    }
                    Ok(Request { id, command: Command::Cast(event) }) => {
                        let origin = Origin::Client { client: client_id, request: id };
                        let error = control
                            .command_tx
                            .send(CastCommand { event, origin })
                            .err()
                            .map(|_| "jellysink is shutting down".to_string());
                        DaemonFrame::Reply { id, error }
                    }
                },
                Ok(None) => return,
                Err(e) => {
                    tracing::debug!(client_id, "dropping a control client: {e}");
                    return;
                }
            },
        };
        if !send(&mut write, client_id, &frame).await {
            return;
        }
    }
}

/// Whether the first line was a hello in our protocol; anything else closes
/// the connection.
async fn greet<R, W>(lines: &mut FrameReader<R>, write: &mut W, client_id: u64) -> bool
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let line = match tokio::time::timeout(HELLO_TIMEOUT, lines.next_line()).await {
        Ok(Ok(Some(line))) => line,
        Ok(Ok(None)) => return false,
        Ok(Err(e)) => {
            tracing::debug!(client_id, "dropping a control client: {e}");
            return false;
        }
        Err(_) => {
            tracing::debug!(client_id, "dropping a control client that sent no hello");
            return false;
        }
    };
    match serde_json::from_str(&line) {
        Ok(Handshake::Hello {
            protocol: PROTOCOL,
            client,
        }) => {
            tracing::debug!(client_id, %client, "control client connected");
            true
        }
        Ok(Handshake::Hello { protocol, client }) => {
            let message =
                format!("{client} speaks protocol {protocol}; this daemon speaks {PROTOCOL}");
            send(write, client_id, &DaemonFrame::Error { message }).await;
            false
        }
        Err(_) => {
            tracing::debug!(
                client_id,
                "dropping a control client whose first line is not a hello"
            );
            false
        }
    }
}

/// `Err` is the frame that answers a line that is not a usable command.
fn parse_request(line: &str) -> Result<Request, DaemonFrame> {
    let value: Value = serde_json::from_str(line).map_err(|e| DaemonFrame::Error {
        message: format!("not JSON: {e}"),
    })?;
    let Some(id) = value.get("id").and_then(Value::as_u64) else {
        return Err(DaemonFrame::Error {
            message: "a command needs a numeric `id`".into(),
        });
    };
    Request::deserialize(value).map_err(|e| DaemonFrame::Reply {
        id,
        error: Some(e.to_string()),
    })
}

/// Whether the client is still worth writing to.
async fn send<W: AsyncWrite + Unpin>(write: &mut W, client_id: u64, frame: &DaemonFrame) -> bool {
    match tokio::time::timeout(WRITE_TIMEOUT, write_frame(write, frame)).await {
        Ok(Ok(())) => true,
        Ok(Err(e)) => {
            tracing::debug!(client_id, "dropping a control client: {e}");
            false
        }
        Err(_) => {
            tracing::warn!(client_id, "dropping a control client that stopped reading");
            false
        }
    }
}

#[cfg(test)]
#[path = "control_test.rs"]
mod tests;
