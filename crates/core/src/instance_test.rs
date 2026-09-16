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
    assert_eq!(
        parse_instance_command("status"),
        Some(InstanceCommand::Status)
    );
    assert_eq!(parse_instance_command("stopping"), None);
    assert_eq!(parse_instance_command("stop-please"), None);
    assert_eq!(parse_instance_command(""), None);
}

#[test]
fn request_status_without_a_running_instance_is_a_usage_error() {
    let tmp = TempDir::new().unwrap();
    let paths = Paths::from_override(Some(tmp.path().to_path_buf())).unwrap();
    let err = request_status(&paths).unwrap_err();
    assert!(err.to_string().contains("not running"));
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

/// The regression: after a SIGKILL the stop socket is left behind, and
/// `is_running` reported true forever.
#[test]
fn a_stale_stop_socket_left_by_a_kill_does_not_look_like_a_running_daemon() {
    let dir = tempfile::TempDir::new().unwrap();
    let paths = Paths::from_override(Some(dir.path().to_path_buf())).unwrap();
    drop(InstanceLock::acquire(&paths).unwrap());
    std::fs::write(paths.stop_socket(), b"").unwrap();
    assert!(paths.stop_socket().exists(), "premise of the test");
    assert!(!is_running(&paths));
}

/// The regression: a socket file left by a SIGKILL refuses the connection, and
/// that surfaced as a color-eyre dump instead of "jellysink is not running".
#[test]
fn a_stale_socket_with_no_listener_reads_as_not_running() {
    let dir = TempDir::new().unwrap();
    let paths = Paths::from_override(Some(dir.path().to_path_buf())).unwrap();
    std::fs::write(paths.stop_socket(), b"").unwrap();
    let err = request_status(&paths).unwrap_err();
    assert!(err.to_string().contains("not running"), "{err:#}");
}

/// Otherwise a daemon killed mid-restart leaves every later call waiting out
/// the full handoff window.
#[test]
fn a_marker_that_outlives_its_daemon_is_cleared_by_the_caller_that_waits_for_it() {
    let dir = TempDir::new().unwrap();
    let paths = Paths::from_override(Some(dir.path().to_path_buf())).unwrap();
    mark_restart_pending(&paths);
    let err = request(&paths, InstanceCommand::Status, Duration::ZERO).unwrap_err();
    assert!(err.to_string().contains("not running"), "{err:#}");
    assert!(!paths.restart_marker().exists());
}
