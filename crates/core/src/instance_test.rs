use super::*;
use crate::config::Paths;
use tempfile::TempDir;

#[test]
fn second_lock_fails() {
    let tmp = TempDir::new().unwrap();
    let paths = Paths::from_override(Some(tmp.path().to_path_buf())).unwrap();
    let _a = InstanceLock::acquire(&paths).unwrap();
    let b = InstanceLock::acquire(&paths);
    assert!(b.is_err());
    assert!(b.unwrap_err().to_string().contains("already running"));
}

#[test]
fn lock_released_on_drop() {
    let tmp = TempDir::new().unwrap();
    let paths = Paths::from_override(Some(tmp.path().to_path_buf())).unwrap();
    {
        let _a = InstanceLock::acquire(&paths).unwrap();
    }
    let _b = InstanceLock::acquire(&paths).unwrap();
}

#[test]
fn no_lock_file_means_nothing_is_running() {
    let dir = tempfile::TempDir::new().unwrap();
    let paths = Paths::from_override(Some(dir.path().to_path_buf())).unwrap();
    assert!(!is_running(&paths));
    assert!(
        !paths.lock_file().exists(),
        "probing must not create the lock file"
    );
}

#[test]
fn a_held_lock_means_something_is_running() {
    let dir = tempfile::TempDir::new().unwrap();
    let paths = Paths::from_override(Some(dir.path().to_path_buf())).unwrap();
    let lock = InstanceLock::acquire(&paths).unwrap();
    assert!(is_running(&paths));
    drop(lock);
    assert!(!is_running(&paths));
}

/// The regression: after a SIGKILL the control socket is left behind, and
/// `is_running` reported true forever.
#[test]
fn a_stale_control_socket_left_by_a_kill_does_not_look_like_a_running_daemon() {
    let dir = tempfile::TempDir::new().unwrap();
    let paths = Paths::from_override(Some(dir.path().to_path_buf())).unwrap();
    drop(InstanceLock::acquire(&paths).unwrap());
    std::fs::write(paths.control_socket(), b"").unwrap();
    assert!(paths.control_socket().exists(), "premise of the test");
    assert!(!is_running(&paths));
}
