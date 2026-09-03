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
mod tests {
    use super::*;
    use crate::config::Paths;
    use tempfile::TempDir;

    #[test]
    fn second_lock_fails() {
        let tmp = TempDir::new().unwrap();
        let paths = Paths {
            config_dir: tmp.path().to_path_buf(),
        };
        let _a = InstanceLock::acquire(&paths).unwrap();
        let b = InstanceLock::acquire(&paths);
        assert!(b.is_err());
        assert!(b.unwrap_err().to_string().contains("already running"));
    }

    #[test]
    fn is_running_false_without_stop_socket() {
        let tmp = TempDir::new().unwrap();
        let paths = Paths {
            config_dir: tmp.path().to_path_buf(),
        };
        assert!(!is_running(&paths));
    }

    #[test]
    fn lock_released_on_drop() {
        let tmp = TempDir::new().unwrap();
        let paths = Paths {
            config_dir: tmp.path().to_path_buf(),
        };
        {
            let _a = InstanceLock::acquire(&paths).unwrap();
        }
        let _b = InstanceLock::acquire(&paths).unwrap();
    }

    #[test]
    fn parse_instance_command_stop_and_restart() {
        assert_eq!(parse_instance_command("stop"), Some(InstanceCommand::Stop));
        assert_eq!(
            parse_instance_command("restart\n"),
            Some(InstanceCommand::Restart)
        );
        assert_eq!(
            parse_instance_command("  restart  "),
            Some(InstanceCommand::Restart)
        );
        assert_eq!(parse_instance_command("stopping"), None);
        assert_eq!(parse_instance_command("stop-please"), None);
        assert_eq!(parse_instance_command(""), None);
    }

    #[test]
    fn no_lock_file_means_nothing_is_running() {
        let dir = tempfile::TempDir::new().unwrap();
        let paths = Paths {
            config_dir: dir.path().to_path_buf(),
        };
        assert!(!is_running(&paths));
        assert!(
            !paths.lock_file().exists(),
            "probing must not create the lock file"
        );
    }

    #[test]
    fn a_held_lock_means_something_is_running() {
        let dir = tempfile::TempDir::new().unwrap();
        let paths = Paths {
            config_dir: dir.path().to_path_buf(),
        };
        let lock = InstanceLock::acquire(&paths).unwrap();
        assert!(is_running(&paths));
        drop(lock);
        assert!(!is_running(&paths));
    }

    /// The regression: after a SIGKILL the stop socket is left behind, and
    /// `is_running` reported true forever.
    #[test]
    fn a_stale_stop_socket_left_by_a_kill_does_not_look_like_a_running_daemon() {
        let dir = tempfile::TempDir::new().unwrap();
        let paths = Paths {
            config_dir: dir.path().to_path_buf(),
        };
        drop(InstanceLock::acquire(&paths).unwrap());
        std::fs::write(paths.stop_socket(), b"").unwrap();
        assert!(paths.stop_socket().exists(), "premise of the test");
        assert!(!is_running(&paths));
    }
}
