use crate::config::Paths;
use crate::usage_err;
use color_eyre::eyre::{WrapErr, eyre};
use rustix::fs::{FlockOperation, flock};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream as StdUnixStream;
use std::sync::Arc;
use tokio::net::UnixListener;
use tokio::sync::Notify;

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

pub async fn listen_stop(paths: &Paths, shutdown: Arc<Notify>) -> color_eyre::Result<()> {
    let sock = paths.stop_socket();
    let _ = std::fs::remove_file(&sock);
    let listener =
        UnixListener::bind(&sock).wrap_err_with(|| format!("binding {}", sock.display()))?;
    if let Ok(meta) = std::fs::metadata(&sock) {
        let mut perms = meta.permissions();
        perms.set_mode(0o600);
        let _ = std::fs::set_permissions(&sock, perms);
    }

    loop {
        tokio::select! {
            _ = shutdown.notified() => break,
            accepted = listener.accept() => {
                match accepted {
                    Ok((mut stream, _)) => {
                        let mut buf = vec![0u8; 64];
                        match tokio::io::AsyncReadExt::read(&mut stream, &mut buf).await {
                            Ok(n) if std::str::from_utf8(&buf[..n]).unwrap_or("").contains("stop") => {
                                tracing::info!("stop requested");
                                shutdown.notify_waiters();
                                break;
                            }
                            _ => {}
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

pub fn is_running(paths: &Paths) -> bool {
    paths.stop_socket().exists()
}

pub fn request_stop(paths: &Paths) -> color_eyre::Result<()> {
    let sock = paths.stop_socket();
    if !sock.exists() {
        return Err(usage_err("jellysink is not running"));
    }
    let mut stream =
        StdUnixStream::connect(&sock).wrap_err("connecting to the running instance")?;
    stream.write_all(b"stop\n")?;
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
}
