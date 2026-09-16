use crate::config::Paths;
use crate::error::usage_err;
use crate::status::PlayerStatus;
use color_eyre::eyre::{WrapErr, eyre};
use rustix::fs::{FlockOperation, flock};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream as StdUnixStream;
use std::time::{Duration, Instant};

/// Long enough to cover a restart: the old daemon reports Stopped and escalates
/// mpv down before it execs, and the new one binds before it does anything slow.
const RESTART_HANDOFF_WAIT: Duration = Duration::from_secs(10);
const RETRY_INTERVAL: Duration = Duration::from_millis(50);

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

impl InstanceCommand {
    const ALL: [Self; 3] = [Self::Stop, Self::Restart, Self::Status];

    fn wire(self) -> &'static str {
        match self {
            Self::Stop => "stop",
            Self::Restart => "restart",
            Self::Status => "status",
        }
    }

    /// The others are answered by acting rather than by writing, so a reader
    /// would wait on a close that shutting down delays.
    fn expects_reply(self) -> bool {
        matches!(self, Self::Status)
    }
}

pub fn parse_instance_command(buf: &str) -> Option<InstanceCommand> {
    InstanceCommand::ALL
        .into_iter()
        .find(|cmd| cmd.wire() == buf.trim())
}

/// Whether another jellysink holds the instance lock — the lock, not the socket
/// file, which a SIGKILL leaves behind.
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

/// `stop.sock` is unbound from the moment a restart is decided until the new
/// image binds; clients wait that out instead of reporting the daemon gone.
pub fn mark_restart_pending(paths: &Paths) {
    if let Err(e) = std::fs::write(paths.restart_marker(), b"") {
        tracing::warn!("could not mark the restart pending: {e}");
    }
}

pub fn clear_restart_pending(paths: &Paths) {
    if let Err(e) = std::fs::remove_file(paths.restart_marker())
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!("could not clear the restart marker: {e}");
    }
}

/// Refused, gone, or accepted and then dropped unanswered: all three are how
/// `stop.sock` behaves while the daemon is between binds.
fn worth_waiting_for(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::NotFound
            | std::io::ErrorKind::ConnectionRefused
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::UnexpectedEof
    )
}

fn exchange(sock: &std::path::Path, cmd: InstanceCommand) -> std::io::Result<Vec<u8>> {
    let mut stream = StdUnixStream::connect(sock)?;
    stream.write_all(format!("{}\n", cmd.wire()).as_bytes())?;
    if !cmd.expects_reply() {
        return Ok(Vec::new());
    }
    stream.shutdown(Shutdown::Write)?;
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf)?;
    if buf.is_empty() {
        // Accepted by a listener that went away before answering.
        return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof));
    }
    Ok(buf)
}

/// The whole exchange is retried, not just the connect: a restart also resets
/// connections it accepted on the way down.
fn request(paths: &Paths, cmd: InstanceCommand, wait: Duration) -> color_eyre::Result<Vec<u8>> {
    let sock = paths.stop_socket();
    let marker = paths.restart_marker();
    let deadline = Instant::now() + wait;
    loop {
        match exchange(&sock, cmd) {
            Ok(buf) => return Ok(buf),
            Err(e) if worth_waiting_for(&e) => {
                if !marker.exists() {
                    return Err(usage_err("jellysink is not running"));
                }
                if Instant::now() >= deadline {
                    // The marker outlived the daemon that wrote it; the next
                    // caller should fail fast rather than wait it out again.
                    clear_restart_pending(paths);
                    return Err(usage_err("jellysink is not running"));
                }
                std::thread::sleep(RETRY_INTERVAL);
            }
            Err(e) => {
                return Err(eyre!(e)).wrap_err_with(|| format!("talking to {}", sock.display()));
            }
        }
    }
}

pub fn request_stop(paths: &Paths) -> color_eyre::Result<()> {
    request(paths, InstanceCommand::Stop, RESTART_HANDOFF_WAIT).map(|_| ())
}

pub fn request_restart(paths: &Paths) -> color_eyre::Result<()> {
    request(paths, InstanceCommand::Restart, RESTART_HANDOFF_WAIT).map(|_| ())
}

/// Asks a running instance what it is doing, over the same socket `stop`/
/// `restart` use — the only request on it that reads a reply back.
pub fn request_status(paths: &Paths) -> color_eyre::Result<PlayerStatus> {
    let buf = request(paths, InstanceCommand::Status, RESTART_HANDOFF_WAIT)?;
    serde_json::from_slice(&buf).wrap_err("parsing status reply")
}

#[cfg(test)]
#[path = "instance_test.rs"]
mod tests;
