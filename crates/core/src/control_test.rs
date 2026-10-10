use super::*;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixListener;

fn wire(frame: impl Serialize) -> Value {
    serde_json::to_value(frame).unwrap()
}

/// The two sides are separate processes, updated separately: these shapes are
/// the whole of what they agree on.
#[test]
fn a_hello_and_its_answers_keep_their_wire_shape() {
    assert_eq!(
        wire(Handshake::Hello {
            protocol: 1,
            client: "jellytui 1.7.0".into()
        }),
        json!({"type": "hello", "protocol": 1, "client": "jellytui 1.7.0"})
    );
    assert_eq!(
        wire(DaemonFrame::Welcome {
            protocol: 1,
            daemon: "1.7.0".into(),
            client_id: 3
        }),
        json!({"type": "welcome", "protocol": 1, "daemon": "1.7.0", "client_id": 3})
    );
    assert_eq!(
        wire(DaemonFrame::Reply { id: 8, error: None }),
        json!({"type": "reply", "id": 8})
    );
    assert_eq!(
        wire(DaemonFrame::Reply {
            id: 9,
            error: Some("unknown command".into())
        }),
        json!({"type": "reply", "id": 9, "error": "unknown command"})
    );
    assert_eq!(
        wire(DaemonFrame::Error {
            message: "not JSON".into()
        }),
        json!({"type": "error", "message": "not JSON"})
    );
    let status = wire(DaemonFrame::Status {
        status: Box::new(PlayerStatus::idle("http://x".into(), "admin".into())),
    });
    assert_eq!(status["type"], "status");
    assert_eq!(status["status"]["username"], "admin");
}

#[test]
fn requests_carry_the_id_beside_the_command() {
    assert_eq!(
        wire(Request {
            id: 1,
            command: Command::Restart
        }),
        json!({"id": 1, "command": "restart"})
    );
    assert_eq!(
        wire(Request {
            id: 9,
            command: Command::Cast(CastEvent::Stop)
        }),
        json!({"id": 9, "command": "stop"})
    );
    let play: Request = serde_json::from_value(json!({
        "id": 8, "command": "play_now", "item_ids": ["f163"], "start_index": 0, "start_ticks": 0
    }))
    .unwrap();
    assert_eq!(
        play,
        Request {
            id: 8,
            command: Command::Cast(CastEvent::PlayNow {
                item_ids: vec!["f163".into()],
                start_index: 0,
                start_ticks: Some(0),
                audio_stream_index: None,
                subtitle_stream_index: None,
                media_source_id: None,
            })
        }
    );
    assert!(
        serde_json::from_value::<Request>(json!({"id": 2, "command": "self_destruct"})).is_err()
    );
}

#[test]
fn events_and_origins_keep_their_wire_shape() {
    let client = Origin::Client {
        client: 3,
        request: 8,
    };
    let event = |event: Event| wire(DaemonFrame::Event(event));
    assert_eq!(
        event(Event::PlayLoading {
            item_id: Some("f163".into()),
            origin: client
        }),
        json!({
            "type": "event", "event": "play_loading", "item_id": "f163",
            "origin": {"client": 3, "request": 8}
        })
    );
    assert_eq!(
        event(Event::PlayStarted {
            item_id: None,
            origin: Origin::Daemon
        }),
        json!({"type": "event", "event": "play_started", "origin": "daemon"})
    );
    assert_eq!(
        event(Event::PlayFailed {
            item_id: Some("f163".into()),
            origin: Origin::Mpris,
            error: "no such item".into()
        }),
        json!({
            "type": "event", "event": "play_failed", "item_id": "f163",
            "origin": "mpris", "error": "no such item"
        })
    );
    assert_eq!(
        event(Event::PlayCancelled {
            item_id: Some("f163".into()),
            origin: client,
            by: Origin::Jellyfin
        }),
        json!({
            "type": "event", "event": "play_cancelled", "item_id": "f163",
            "origin": {"client": 3, "request": 8}, "by": "jellyfin"
        })
    );
    assert_eq!(
        event(Event::Reported {
            report: ReportKind::Stopped,
            item_id: "f163".into(),
            ok: true
        }),
        json!({"type": "event", "event": "reported", "report": "stopped", "item_id": "f163", "ok": true})
    );
    for origin in [client, Origin::Jellyfin, Origin::Mpris, Origin::Daemon] {
        assert_eq!(
            serde_json::from_value::<Origin>(wire(origin)).unwrap(),
            origin
        );
    }
}

/// A newer daemon adds fields; an older client must still read the frame.
#[test]
fn a_frame_with_fields_this_side_does_not_know_still_parses() {
    let welcome: DaemonFrame = serde_json::from_value(json!({
        "type": "welcome", "protocol": 1, "daemon": "1.9.0", "client_id": 3, "features": ["queue"]
    }))
    .unwrap();
    assert!(matches!(welcome, DaemonFrame::Welcome { client_id: 3, .. }));
    let started: DaemonFrame = serde_json::from_value(json!({
        "type": "event", "event": "play_started", "origin": "jellyfin", "queue_index": 4
    }))
    .unwrap();
    assert!(matches!(
        started,
        DaemonFrame::Event(Event::PlayStarted {
            origin: Origin::Jellyfin,
            ..
        })
    ));
}

#[test]
fn only_a_hello_opens_a_connection() {
    let parse = |line: &str| serde_json::from_str::<Handshake>(line);
    assert!(parse(r#"{"type":"hello","protocol":1,"client":"x"}"#).is_ok());
    assert!(parse(r#"{"protocol":1,"client":"x"}"#).is_err());
    assert!(parse(r#"{"type":"welcome","protocol":1,"client":"x"}"#).is_err());
}

fn paths(dir: &TempDir) -> Paths {
    Paths::from_override(Some(dir.path().to_path_buf())).unwrap()
}

/// The regression: a socket file left by a SIGKILL refuses the connection, and
/// that surfaced as a color-eyre dump instead of "jellysink is not running".
#[tokio::test]
async fn a_stale_socket_with_no_listener_reads_as_not_running() {
    let dir = TempDir::new().unwrap();
    let paths = paths(&dir);
    std::fs::write(paths.control_socket(), b"").unwrap();
    let err = Client::connect(&paths, "test").await.unwrap_err();
    assert!(err.to_string().contains("not running"), "{err:#}");
}

/// The regression: with no read timeout a listener that accepts and never
/// answers hung the caller.
#[tokio::test]
async fn a_listener_that_never_answers_reads_as_not_responding() {
    let dir = TempDir::new().unwrap();
    let paths = paths(&dir);
    let listener = UnixListener::bind(paths.control_socket()).unwrap();
    let accepting = tokio::spawn(async move { listener.accept().await.map(|(stream, _)| stream) });

    let err = Client::connect(&paths, "test").await.unwrap_err();
    assert!(err.to_string().contains("not responding"), "{err:#}");
    drop(accepting.await.unwrap().unwrap());
}

/// A daemon newer than this client sends frames it has no type for.
#[tokio::test]
async fn frames_of_an_unknown_type_or_event_are_skipped() {
    let dir = TempDir::new().unwrap();
    let paths = paths(&dir);
    let listener = UnixListener::bind(paths.control_socket()).unwrap();
    let daemon = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (read, mut write) = stream.into_split();
        FrameReader::new(read).next_line().await.unwrap().unwrap();
        for frame in [
            json!({"type": "welcome", "protocol": 1, "daemon": "9.0.0", "client_id": 1}),
            json!({"type": "status", "status": {
                "server": "s", "username": "u", "now_playing": null, "loading": false,
                "server_connected": true
            }}),
            json!({"type": "queue_changed", "queue": []}),
            json!({"type": "event", "event": "volume_changed", "volume": 40}),
            json!({"type": "reply", "id": 7}),
        ] {
            write_frame(&mut write, &frame).await.unwrap();
        }
        write
    });

    let (mut client, status) = Client::connect(&paths, "test").await.unwrap();
    assert_eq!(status.username, "u");
    assert!(matches!(
        client.next_frame().await.unwrap(),
        Some(DaemonFrame::Reply { id: 7, error: None })
    ));
    drop(daemon.await.unwrap());
}

/// jellytui reads in a `select!`, which drops a read that lost the race.
#[tokio::test]
async fn a_read_cut_short_keeps_what_it_had() {
    let (mut daemon, client) = tokio::io::duplex(1024);
    let mut lines = FrameReader::new(client);
    daemon.write_all(br#"{"type":"#).await.unwrap();
    tokio::select! {
        biased;
        _ = lines.next_line() => panic!("half a line came back as a line"),
        () = std::future::ready(()) => {}
    }
    daemon.write_all(b"\"reply\",\"id\":1}\n").await.unwrap();
    assert_eq!(
        lines.next_line().await.unwrap().as_deref(),
        Some(r#"{"type":"reply","id":1}"#)
    );
}

#[tokio::test]
async fn a_line_over_the_limit_is_an_error() {
    let (mut daemon, client) = tokio::io::duplex(MAX_LINE * 2);
    daemon.write_all(&vec![b'x'; MAX_LINE + 1]).await.unwrap();
    let err = FrameReader::new(client).next_line().await.unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}
