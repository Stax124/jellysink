use crate::app::config::Paths;
use crate::app::signal::Signal;
use crate::runtime::PlayerStatus;
use crate::usage_err;
use color_eyre::eyre::{WrapErr, eyre};
use rustix::fs::{FlockOperation, flock};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream as StdUnixStream;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixListener;

#[derive(Debug)]
pub(crate) struct InstanceLock {
    _file: File,
}

impl InstanceLock {
    pub(crate) fn acquire(paths: &Paths) -> color_eyre::Result<Self> {
        paths.ensure()?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(paths.lock_file())
            .wrap_err("opening instance.lock")?;
        match flock(&file, FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => Ok(Self { _file: file }),
            Err(e) if e == rustix::io::Errno::WOULDBLOCK => Err(usage_err(
                "jellysink is already running (use `jellysink stop`)",
            )),
            Err(e) => Err(eyre!(e).wrap_err("locking instance.lock")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InstanceCommand {
    Stop,
    Restart,
    Status,
}

pub(crate) fn parse_instance_command(buf: &str) -> Option<InstanceCommand> {
    match buf.trim() {
        "stop" => Some(InstanceCommand::Stop),
        "restart" => Some(InstanceCommand::Restart),
        "status" => Some(InstanceCommand::Status),
        _ => None,
    }
}

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

/// Whether another jellysink currently holds the instance lock. The lock, not
/// the socket file, which a SIGKILL leaves behind — the kernel releases a flock
/// when its holder dies.
pub(crate) fn is_running(paths: &Paths) -> bool {
    // Not `create(true)`: probing should not leave a lock file behind.
    let Ok(file) = File::open(paths.lock_file()) else {
        return false;
    };
    match flock(&file, FlockOperation::NonBlockingLockExclusive) {
        // We took it, so nobody else holds it; dropping `file` releases it.
        Ok(()) => false,
        Err(e) if e == rustix::io::Errno::WOULDBLOCK => true,
        // Can't tell; do not send the caller off to stop a maybe-daemon.
        Err(e) => {
            tracing::debug!("probing instance.lock: {e}");
            false
        }
    }
}

pub(crate) fn request_stop(paths: &Paths) -> color_eyre::Result<()> {
    write_instance_command(paths, b"stop\n")
}

pub(crate) fn request_restart(paths: &Paths) -> color_eyre::Result<()> {
    write_instance_command(paths, b"restart\n")
}

/// Asks a running instance what it is doing, over the same socket `stop`/
/// `restart` use — the only request on it that reads a reply back.
pub(crate) fn request_status(paths: &Paths) -> color_eyre::Result<PlayerStatus> {
    let sock = paths.stop_socket();
    if !sock.exists() {
        return Err(usage_err("jellysink is not running"));
    }
    let mut stream =
        StdUnixStream::connect(&sock).wrap_err("connecting to the running instance")?;
    stream
        .write_all(b"status\n")
        .wrap_err("sending status request to the running instance")?;
    stream
        .shutdown(Shutdown::Write)
        .wrap_err("closing write half")?;
    let mut buf = Vec::new();
    stream
        .read_to_end(&mut buf)
        .wrap_err("reading status reply")?;
    serde_json::from_slice(&buf).wrap_err("parsing status reply")
}

fn write_instance_command(paths: &Paths, msg: &[u8]) -> color_eyre::Result<()> {
    let sock = paths.stop_socket();
    if !sock.exists() {
        return Err(usage_err("jellysink is not running"));
    }
    let mut stream =
        StdUnixStream::connect(&sock).wrap_err("connecting to the running instance")?;
    stream
        .write_all(msg)
        .wrap_err_with(|| format!("writing to {}", sock.display()))?;
    Ok(())
}

#[cfg(test)]
#[path = "instance_test.rs"]
mod tests;
