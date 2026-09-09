//! The instance lock and the `stop.sock` client: `jellysink stop`, the tray's
//! restart, and the status poll both the CLI and jellytui make.

use crate::config::Paths;
use crate::error::usage_err;
use crate::status::PlayerStatus;
use color_eyre::eyre::{WrapErr, eyre};
use rustix::fs::{FlockOperation, flock};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream as StdUnixStream;

#[derive(Debug)]
pub struct InstanceLock {
    _file: File,
}

impl InstanceLock {
    pub fn acquire(paths: &Paths) -> color_eyre::Result<Self> {
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
pub enum InstanceCommand {
    Stop,
    Restart,
    Status,
}

pub fn parse_instance_command(buf: &str) -> Option<InstanceCommand> {
    match buf.trim() {
        "stop" => Some(InstanceCommand::Stop),
        "restart" => Some(InstanceCommand::Restart),
        "status" => Some(InstanceCommand::Status),
        _ => None,
    }
}

/// Whether another jellysink currently holds the instance lock. The lock, not
/// the socket file, which a SIGKILL leaves behind — the kernel releases a flock
/// when its holder dies.
pub fn is_running(paths: &Paths) -> bool {
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

pub fn request_stop(paths: &Paths) -> color_eyre::Result<()> {
    write_instance_command(paths, b"stop\n")
}

pub fn request_restart(paths: &Paths) -> color_eyre::Result<()> {
    write_instance_command(paths, b"restart\n")
}

/// Asks a running instance what it is doing, over the same socket `stop`/
/// `restart` use — the only request on it that reads a reply back.
pub fn request_status(paths: &Paths) -> color_eyre::Result<PlayerStatus> {
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
