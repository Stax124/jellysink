use crate::config::Paths;
use crate::error::usage_err;
use color_eyre::eyre::{WrapErr, eyre};
use rustix::fs::{FlockOperation, flock};
use std::fs::{File, OpenOptions};

#[derive(Debug)]
pub struct InstanceLock {
    _file: File,
}

impl InstanceLock {
    pub fn acquire(paths: &Paths) -> color_eyre::Result<Self> {
        paths.ensure()?;
        let lock_file = paths.lock_file();
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_file)
            .wrap_err_with(|| format!("opening {}", lock_file.display()))?;
        match flock(&file, FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => Ok(Self { _file: file }),
            Err(e) if e == rustix::io::Errno::WOULDBLOCK => Err(usage_err(
                "jellysink is already running (quit it from the tray or with `systemctl --user stop jellysink`)",
            )),
            Err(e) => Err(eyre!(e).wrap_err(format!("locking {}", lock_file.display()))),
        }
    }
}

/// Whether another jellysink holds the instance lock — the lock, not the socket
/// file, which a SIGKILL leaves behind.
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

#[cfg(test)]
#[path = "instance_test.rs"]
mod tests;
