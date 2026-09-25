use super::{bind_stop_socket, listen_stop};
use crate::daemon::signal::Signal;
use jellysink_core::config::Paths;
use jellysink_core::instance::request_status;
use jellysink_core::status::PlayerStatus;
use std::path::PathBuf;
use std::time::Duration;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::task::JoinHandle;

fn serve(sock: PathBuf, shutdown: &Signal) -> JoinHandle<()> {
    let (_status_tx, status_rx) = tokio::sync::watch::channel(PlayerStatus::idle(
        "http://jelly.example".into(),
        "admin".into(),
    ));
    // Bound outside the task so the path exists before the client connects; the
    // task is what answers on it.
    let listener = bind_stop_socket(&sock).unwrap();
    tokio::spawn(listen_stop(
        listener,
        sock,
        shutdown.clone(),
        Signal::new(),
        status_rx,
    ))
}

#[tokio::test]
async fn status_round_trips_over_the_socket() {
    let tmp = TempDir::new().unwrap();
    let paths = Paths::from_override(Some(tmp.path().to_path_buf())).unwrap();
    let shutdown = Signal::new();
    let serving = serve(paths.stop_socket(), &shutdown);

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
    serving.await.unwrap();

    // The teardown window depends on this: while the daemon escalates mpv down
    // nothing accepts, and a socket left behind stalls every client until it.
    assert!(
        !paths.stop_socket().exists(),
        "the listener must take its path with it"
    );
}

/// The regression: a client that connected and never wrote stalled the listener
/// on its read, so every later `status` timed out as "not responding".
#[tokio::test]
async fn a_client_that_sends_nothing_does_not_block_the_next_one() {
    let tmp = TempDir::new().unwrap();
    let sock = tmp.path().join("stop.sock");
    let shutdown = Signal::new();
    let serving = serve(sock.clone(), &shutdown);

    let _silent = UnixStream::connect(&sock).await.unwrap();
    let mut client = UnixStream::connect(&sock).await.unwrap();
    client.write_all(b"status\n").await.unwrap();
    let mut reply = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), client.read_to_end(&mut reply))
        .await
        .expect("the silent client stalled the listener")
        .unwrap();
    let status: PlayerStatus = serde_json::from_slice(&reply).unwrap();
    assert_eq!(status.username, "admin");

    shutdown.fire();
    serving.await.unwrap();
}
