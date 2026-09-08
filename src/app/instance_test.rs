use super::*;
use crate::app::config::Paths;
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
    assert_eq!(
        parse_instance_command("status"),
        Some(InstanceCommand::Status)
    );
    assert_eq!(parse_instance_command("stopping"), None);
    assert_eq!(parse_instance_command("stop-please"), None);
    assert_eq!(parse_instance_command(""), None);
}

#[tokio::test]
async fn status_round_trips_over_the_socket() {
    let tmp = TempDir::new().unwrap();
    let paths = Paths {
        config_dir: tmp.path().to_path_buf(),
    };
    let shutdown = Signal::new();
    let restart = Signal::new();
    let (_status_tx, status_rx) = tokio::sync::watch::channel(PlayerStatus::idle(
        "http://jelly.example".into(),
        "admin".into(),
    ));

    let listen_paths = paths.clone();
    let listen_shutdown = shutdown.clone();
    let listener = tokio::spawn(async move {
        listen_stop(&listen_paths, listen_shutdown, restart, status_rx).await
    });

    while !paths.stop_socket().exists() {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }

    let status = tokio::task::spawn_blocking({
        let paths = paths.clone();
        move || request_status(&paths)
    })
    .await
    .unwrap()
    .unwrap();

    assert_eq!(status.server, "http://jelly.example");
    assert_eq!(status.username, "admin");
    assert!(status.now_playing.is_none());

    shutdown.fire();
    listener.await.unwrap().unwrap();
}

#[test]
fn request_status_without_a_running_instance_is_a_usage_error() {
    let tmp = TempDir::new().unwrap();
    let paths = Paths {
        config_dir: tmp.path().to_path_buf(),
    };
    let err = request_status(&paths).unwrap_err();
    assert!(err.to_string().contains("not running"));
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
