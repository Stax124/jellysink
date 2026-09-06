use crate::config::Paths;
use crate::signal::Signal;
use crate::usage_err;
use color_eyre::eyre::{WrapErr, eyre};
use rustix::fs::{FlockOperation, flock};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream as StdUnixStream;
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
}

pub(crate) fn parse_instance_command(buf: &str) -> Option<InstanceCommand> {
    match buf.trim() {
        "stop" => Some(InstanceCommand::Stop),
        "restart" => Some(InstanceCommand::Restart),
        _ => None,
    }
}

pub(crate) async fn listen_stop(
    paths: &Paths,
    shutdown: Signal,
    restart: Signal,
) -> color_eyre::Result<()> {
    let sock = paths.stop_socket();
    let _ = std::fs::remove_file(&sock);
    let listener =
        UnixListener::bind(&sock).wrap_err_with(|| format!("binding {}", sock.display()))?;
    // A bound socket cannot be given a mode up front, so there is a window at
    // the ambient umask here. `Paths::ensure` makes the directory 0700, which is
    // what actually keeps this private; the chmod is defence in depth.
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

/// Whether another jellysink currently holds the instance lock.
///
/// This used to be `stop_socket().exists()`, which is not a liveness signal: a
/// SIGKILL leaves the socket file behind, so `is_running` stayed true forever
/// and `jellysink update` then chose the stop path and failed with "connecting
/// to the running instance". The kernel releases a flock when the holder dies,
/// so the lock is authoritative.
pub(crate) fn is_running(paths: &Paths) -> bool {
    // Deliberately not `create(true)`: no lock file means jellysink has never
    // run here, and probing should not leave one behind.
    let Ok(file) = File::open(paths.lock_file()) else {
        return false;
    };
    match flock(&file, FlockOperation::NonBlockingLockExclusive) {
        // We took it, so nobody else holds it. Dropping `file` releases it.
        Ok(()) => false,
        Err(e) if e == rustix::io::Errno::WOULDBLOCK => true,
        // Can't tell. Say no, so the caller does not try to stop a daemon that
        // may not be there.
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

fn write_instance_command(paths: &Paths, msg: &[u8]) -> color_eyre::Result<()> {
    let sock = paths.stop_socket();
    if !sock.exists() {
        return Err(usage_err("jellysink is not running"));
    }
    let mut stream =
        StdUnixStream::connect(&sock).wrap_err("connecting to the running instance")?;
    stream.write_all(msg)?;
    Ok(())
}

#[cfg(test)]
#[path = "instance_test.rs"]
mod tests;
