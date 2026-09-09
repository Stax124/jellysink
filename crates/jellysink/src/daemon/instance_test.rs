use super::listen_stop;
use crate::daemon::signal::Signal;
use jellysink_core::config::Paths;
use jellysink_core::instance::request_status;
use jellysink_core::status::PlayerStatus;
use tempfile::TempDir;

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
