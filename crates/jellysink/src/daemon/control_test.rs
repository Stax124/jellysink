use super::{Control, bind_control_socket, listen_control, serve};
use crate::daemon::signal::Signal;
use crate::runtime::CastCommand;
use jellysink_core::cast::CastEvent;
use jellysink_core::config::Paths;
use jellysink_core::control::{
    Client, Command, DaemonFrame, Event, FrameReader, Handshake, Origin, PROTOCOL, Request,
    write_frame,
};
use jellysink_core::status::PlayerStatus;
use std::time::Duration;
use tempfile::TempDir;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::net::UnixStream;
use tokio::sync::{broadcast, mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::timeout;

const PATIENCE: Duration = Duration::from_secs(5);

fn idle() -> PlayerStatus {
    PlayerStatus {
        server_connected: true,
        ..PlayerStatus::idle("http://jelly.example".into(), "admin".into())
    }
}

struct Harness {
    paths: Paths,
    status_tx: watch::Sender<PlayerStatus>,
    event_tx: broadcast::Sender<Event>,
    command_rx: mpsc::UnboundedReceiver<CastCommand>,
    restart: Signal,
    shutdown: Signal,
    listening: JoinHandle<()>,
    _dir: TempDir,
}

impl Harness {
    fn start() -> Self {
        let dir = TempDir::new().unwrap();
        let paths = Paths::from_override(Some(dir.path().to_path_buf())).unwrap();
        let (status_tx, status_rx) = watch::channel(idle());
        let (event_tx, _) = broadcast::channel(16);
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (restart, shutdown) = (Signal::new(), Signal::new());
        let sock = paths.control_socket();
        // Bound outside the task so the path exists before a client connects.
        let listener = bind_control_socket(&sock).unwrap();
        let listening = tokio::spawn(listen_control(
            listener,
            sock,
            shutdown.clone(),
            Control {
                status_rx,
                event_tx: event_tx.clone(),
                command_tx,
                restart: restart.clone(),
            },
        ));
        Self {
            paths,
            status_tx,
            event_tx,
            command_rx,
            restart,
            shutdown,
            listening,
            _dir: dir,
        }
    }

    async fn client(&self) -> Client {
        Client::connect(&self.paths, "test").await.unwrap().0
    }

    async fn stop(self) -> Paths {
        self.shutdown.fire();
        self.listening.await.unwrap();
        self.paths
    }
}

async fn next(client: &mut Client) -> DaemonFrame {
    timeout(PATIENCE, client.next_frame())
        .await
        .expect("no frame within 5 s")
        .unwrap()
        .expect("the daemon closed the connection")
}

async fn parse<R: AsyncRead + Unpin>(lines: &mut FrameReader<R>) -> DaemonFrame {
    let line = timeout(PATIENCE, lines.next_line())
        .await
        .expect("no frame within 5 s")
        .unwrap()
        .expect("the daemon closed the connection");
    serde_json::from_str(&line).unwrap()
}

/// A client below `Client`, for lines `Client` would never send.
async fn greeted<S: AsyncRead + AsyncWrite>(
    stream: S,
) -> (FrameReader<ReadHalf<S>>, WriteHalf<S>, u64) {
    let (read, mut write) = tokio::io::split(stream);
    let mut lines = FrameReader::new(read);
    let hello = Handshake::Hello {
        protocol: PROTOCOL,
        client: "raw".into(),
    };
    write_frame(&mut write, &hello).await.unwrap();
    let DaemonFrame::Welcome { client_id, .. } = parse(&mut lines).await else {
        panic!("no welcome");
    };
    assert!(matches!(
        parse(&mut lines).await,
        DaemonFrame::Status { .. }
    ));
    (lines, write, client_id)
}

#[tokio::test]
async fn a_hello_is_welcomed_with_a_snapshot_and_then_every_change() {
    let daemon = Harness::start();
    let (mut client, status) = Client::connect(&daemon.paths, "test").await.unwrap();
    assert_eq!(status.username, "admin");
    assert!(!status.loading);

    daemon.status_tx.send_modify(|status| status.loading = true);
    assert!(matches!(next(&mut client).await, DaemonFrame::Status { status } if status.loading));

    let paths = daemon.stop().await;
    // The teardown window depends on this: while the daemon escalates mpv down
    // nothing accepts, and a socket left behind stalls every client until it.
    assert!(
        !paths.control_socket().exists(),
        "the listener must take its path with it"
    );
}

/// `status` is what a client from before this protocol sends.
#[tokio::test]
async fn a_first_line_that_is_not_a_hello_is_closed() {
    let daemon = Harness::start();
    for first in ["status\n", "{\"id\":1,\"command\":\"stop\"}\n"] {
        let mut stream = UnixStream::connect(daemon.paths.control_socket())
            .await
            .unwrap();
        stream.write_all(first.as_bytes()).await.unwrap();
        let mut answer = Vec::new();
        timeout(PATIENCE, stream.read_to_end(&mut answer))
            .await
            .expect("the connection was left open")
            .unwrap();
        assert!(answer.is_empty(), "{first:?} was answered: {answer:?}");
    }
}

#[tokio::test]
async fn a_hello_in_another_protocol_is_refused_with_a_reason() {
    let daemon = Harness::start();
    let stream = UnixStream::connect(daemon.paths.control_socket())
        .await
        .unwrap();
    let (read, mut write) = stream.into_split();
    let hello = Handshake::Hello {
        protocol: PROTOCOL + 1,
        client: "future".into(),
    };
    write_frame(&mut write, &hello).await.unwrap();
    let mut lines = FrameReader::new(read);
    assert!(matches!(parse(&mut lines).await, DaemonFrame::Error { .. }));
    assert!(lines.next_line().await.unwrap().is_none());
}

/// The regression: a client that connected and never wrote stalled the listener
/// on its read, so every later `status` timed out as "not responding".
#[tokio::test]
async fn a_client_that_sends_nothing_does_not_block_the_next_one() {
    let daemon = Harness::start();
    let _silent = UnixStream::connect(daemon.paths.control_socket())
        .await
        .unwrap();
    let (_, status) = timeout(PATIENCE, Client::connect(&daemon.paths, "test"))
        .await
        .expect("the silent client stalled the listener")
        .unwrap();
    assert_eq!(status.username, "admin");
}

#[tokio::test]
async fn restart_is_replied_to_and_then_fires() {
    let daemon = Harness::start();
    let mut client = daemon.client().await;
    let restart = Request {
        id: 1,
        command: Command::Restart,
    };
    client.send(&restart).await.unwrap();
    assert!(matches!(
        next(&mut client).await,
        DaemonFrame::Reply { id: 1, error: None }
    ));
    timeout(PATIENCE, daemon.restart.fired())
        .await
        .expect("the restart never fired");
}

#[tokio::test]
async fn two_clients_both_receive_a_status_change_and_an_event() {
    let daemon = Harness::start();
    let (mut first, mut second) = (daemon.client().await, daemon.client().await);
    assert_ne!(first.id(), second.id());

    daemon.status_tx.send_modify(|status| status.loading = true);
    let loading = Event::PlayLoading {
        item_id: Some("e1".into()),
        origin: Origin::Client {
            client: first.id(),
            request: 1,
        },
    };
    daemon.event_tx.send(loading.clone()).unwrap();
    for client in [&mut first, &mut second] {
        assert!(matches!(next(client).await, DaemonFrame::Status { status } if status.loading));
        assert!(matches!(next(client).await, DaemonFrame::Event(event) if event == loading));
    }
}

#[tokio::test]
async fn a_malformed_line_is_answered_and_the_connection_survives() {
    let mut daemon = Harness::start();
    let stream = UnixStream::connect(daemon.paths.control_socket())
        .await
        .unwrap();
    let (mut lines, mut write, client) = greeted(stream).await;

    write.write_all(b"not json\n").await.unwrap();
    assert!(matches!(parse(&mut lines).await, DaemonFrame::Error { .. }));
    write.write_all(b"{\"command\":\"stop\"}\n").await.unwrap();
    assert!(matches!(parse(&mut lines).await, DaemonFrame::Error { .. }));
    write
        .write_all(b"{\"id\":5,\"command\":\"self_destruct\"}\n")
        .await
        .unwrap();
    assert!(matches!(
        parse(&mut lines).await,
        DaemonFrame::Reply {
            id: 5,
            error: Some(_)
        }
    ));
    write
        .write_all(b"{\"id\":6,\"command\":\"restart\"}\n")
        .await
        .unwrap();
    assert!(matches!(
        parse(&mut lines).await,
        DaemonFrame::Reply { id: 6, error: None }
    ));
    write
        .write_all(b"{\"id\":7,\"command\":\"stop\"}\n")
        .await
        .unwrap();
    assert!(matches!(
        parse(&mut lines).await,
        DaemonFrame::Reply { id: 7, error: None }
    ));
    assert_eq!(
        daemon.command_rx.try_recv().unwrap(),
        CastCommand {
            event: CastEvent::Stop,
            origin: Origin::Client { client, request: 7 }
        }
    );
}

/// `run_session` reads commands, so one accepted with no session would wait for
/// the next one and play long after the client gave up.
#[tokio::test]
async fn a_cast_is_refused_while_the_daemon_has_no_server_session() {
    let mut daemon = Harness::start();
    daemon
        .status_tx
        .send_modify(|status| status.server_connected = false);
    let mut client = daemon.client().await;
    let stop = |id| Request {
        id,
        command: Command::Cast(CastEvent::Stop),
    };
    client.send(&stop(1)).await.unwrap();
    assert!(matches!(
        next(&mut client).await,
        DaemonFrame::Reply {
            id: 1,
            error: Some(_)
        }
    ));
    assert!(daemon.command_rx.try_recv().is_err());

    daemon
        .status_tx
        .send_modify(|status| status.server_connected = true);
    assert!(matches!(
        next(&mut client).await,
        DaemonFrame::Status { .. }
    ));
    client.send(&stop(2)).await.unwrap();
    assert!(matches!(
        next(&mut client).await,
        DaemonFrame::Reply { id: 2, error: None }
    ));
    assert_eq!(daemon.command_rx.try_recv().unwrap().event, CastEvent::Stop);
}

#[tokio::test]
async fn a_client_that_stops_reading_is_dropped_and_the_others_keep_receiving() {
    let (_status_tx, status_rx) = watch::channel(idle());
    let (event_tx, _) = broadcast::channel(2);
    let (command_tx, _command_rx) = mpsc::unbounded_channel();
    let control = Control {
        status_rx,
        event_tx: event_tx.clone(),
        command_tx,
        restart: Signal::new(),
    };
    // Smaller than one event, so the first one it does not read leaves its
    // task stuck in a write.
    let (slow, slow_daemon) = tokio::io::duplex(64);
    let (fast, fast_daemon) = tokio::io::duplex(64 * 1024);
    let slow_task = tokio::spawn(serve(slow_daemon, 1, control.clone()));
    tokio::spawn(serve(fast_daemon, 2, control));
    let (mut slow, _slow_write, _) = greeted(slow).await;
    let (mut fast, _fast_write, _) = greeted(fast).await;

    let loading = |request| Event::PlayLoading {
        item_id: Some("00000000000000000000000000000000".into()),
        origin: Origin::Client { client: 2, request },
    };
    for request in 0..8 {
        event_tx.send(loading(request)).unwrap();
        assert!(
            matches!(parse(&mut fast).await, DaemonFrame::Event(event) if event == loading(request))
        );
    }

    timeout(PATIENCE, async {
        while slow.next_line().await.unwrap().is_some() {}
    })
    .await
    .expect("the client that stopped reading was never dropped");
    timeout(PATIENCE, slow_task).await.unwrap().unwrap();

    event_tx.send(loading(8)).unwrap();
    assert!(matches!(parse(&mut fast).await, DaemonFrame::Event(event) if event == loading(8)));
}

#[tokio::test]
async fn a_seventeenth_client_is_turned_away_until_one_leaves() {
    let daemon = Harness::start();
    let mut clients = Vec::new();
    for _ in 0..16 {
        clients.push(daemon.client().await);
    }
    let refused = Client::connect(&daemon.paths, "test").await.unwrap_err();
    assert!(refused.to_string().contains("not running"), "{refused:#}");

    drop(clients.pop());
    // Its task ends once it reads the close, which nothing here can wait on.
    let mut attempts = 0;
    while Client::connect(&daemon.paths, "test").await.is_err() {
        attempts += 1;
        assert!(attempts < 50, "a departed client never freed its slot");
        tokio::task::yield_now().await;
    }
}
