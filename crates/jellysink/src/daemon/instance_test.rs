use super::{bind_stop_socket, listen_stop};
use crate::daemon::signal::Signal;
use jellysink_core::config::Paths;
use jellysink_core::instance::request_status;
use jellysink_core::status::PlayerStatus;
use tempfile::TempDir;

#[tokio::test]
async fn status_round_trips_over_the_socket() {
    let tmp = TempDir::new().unwrap();
    let paths = Paths::from_override(Some(tmp.path().to_path_buf())).unwrap();
    let shutdown = Signal::new();
    let restart = Signal::new();
    let (_status_tx, status_rx) = tokio::sync::watch::channel(PlayerStatus::idle(
        "http://jelly.example".into(),
        "admin".into(),
    ));

    // Bound outside the task so the path exists before the client connects; the
    // task is what answers on it.
    let listener = bind_stop_socket(&paths).unwrap();
    let listen_shutdown = shutdown.clone();
    let listen_paths = paths.clone();
    let serving = tokio::spawn(async move {
        listen_stop(listener, listen_paths, listen_shutdown, restart, status_rx).await
    });

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
    serving.await.unwrap().unwrap();

    // The teardown window depends on this: while the daemon escalates mpv down
    // nothing accepts, and a socket left behind stalls every client until it.
    assert!(
        !paths.stop_socket().exists(),
        "the listener must take its path with it"
    );
}
