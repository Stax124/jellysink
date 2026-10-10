//! The daemon against a live Jellyfin: the one `testserver/start.sh` starts.
//! Headless and `--no-config` mpv, as in `mpv/integration_test.rs`.

use super::run;
use crate::daemon::control::{Control, EVENT_CAPACITY, bind_control_socket, listen_control};
use crate::daemon::signal::Signal;
use jellysink_core::cast::CastEvent;
use jellysink_core::config::{Config, Credentials, Field, Paths};
use jellysink_core::control::{Client, Command, DaemonFrame, Event, Origin, ReportKind, Request};
use jellysink_core::jellyfin::auth::{Api, login};
use jellysink_core::jellyfin::browse::ItemQuery;
use jellysink_core::jellyfin::model::{Item, ItemList};
use jellysink_core::jellyfin::session::websocket_url;
use jellysink_core::status::PlayerStatus;
use serde::Deserialize;
use serde_json::Value;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, mpsc, watch};

const USERNAME: &str = "jellysink";
const PASSWORD: &str = "jellysink";
const SERIES: &str = "That Time I Got Reincarnated as a Slime";

/// Generous so a loaded CI box does not fail the suite.
const SETTLE: Duration = Duration::from_secs(30);

async fn test_credentials() -> Credentials {
    jellysink_core::install_crypto_provider();
    let port = std::env::var("JELLYSINK_TEST_PORT").unwrap_or_else(|_| "8096".to_string());
    let server = format!("http://127.0.0.1:{port}");
    let device_id = uuid::Uuid::new_v4().to_string();
    login(&server, USERNAME, PASSWORD, &device_id)
        .await
        .unwrap_or_else(|e| {
            panic!(
                "no Jellyfin test server at {server} ({e:#}); start one with `testserver/start.sh`"
            )
        })
}

async fn first_episode(api: &Api) -> Item {
    let query = ItemQuery::search("Slime").with_types("Series");
    let listing = api.items(&query).await.unwrap();
    let series = ItemList::deserialize(listing)
        .unwrap()
        .items
        .into_iter()
        .find(|item| item.name.as_deref() == Some(SERIES))
        .expect("the test library's series");
    ItemList::deserialize(api.episodes_all(&series.id).await.unwrap())
        .unwrap()
        .items
        .into_iter()
        .find(|episode| episode.parent_index_number == Some(1) && episode.index_number == Some(1))
        .expect("S01E01")
}

fn authorization(api: &Api) -> String {
    let header = api.mpv_auth_header_field();
    header.strip_prefix("Authorization: ").unwrap().to_string()
}

/// The daemon's session once the server has a live WebSocket for this device: a
/// login alone already lists it, just without remote control.
async fn remote_session(api: &Api) -> Option<String> {
    let sessions: Vec<Value> = reqwest::Client::new()
        .get(format!(
            "{}/Sessions?deviceId={}",
            api.server, api.device_id
        ))
        .header("Authorization", authorization(api))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    sessions
        .iter()
        .find(|session| session["SupportsRemoteControl"] == true)
        .and_then(|session| session["Id"].as_str())
        .map(str::to_string)
}

/// Guards the token going out as `api_key`, which the server answers with a 403.
#[tokio::test]
async fn the_server_accepts_the_websocket_url() {
    let api = Api::from_credentials(&test_credentials().await).unwrap();
    let url = websocket_url(&api.server, &api.token, &api.device_id).unwrap();
    tokio_tungstenite::connect_async(&url)
        .await
        .expect("connecting the websocket");
}

/// `run` and its control socket against the test server.
struct LiveDaemon {
    status_rx: watch::Receiver<PlayerStatus>,
    shutdown: Signal,
    daemon: tokio::task::JoinHandle<color_eyre::Result<()>>,
    api: Api,
    session_id: String,
    paths: Paths,
    _dir: tempfile::TempDir,
}

impl LiveDaemon {
    async fn start() -> Self {
        let creds = test_credentials().await;
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::from_override(Some(dir.path().to_path_buf())).unwrap();
        Field::MpvArgs
            .write(&paths, "--no-config --vo=null --ao=null --really-quiet")
            .unwrap();
        let shutdown = Signal::new();
        let (status_tx, status_rx) = watch::channel(PlayerStatus::idle(
            creds.server.clone(),
            creds.username.clone(),
        ));
        let (event_tx, _) = broadcast::channel(EVENT_CAPACITY);
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let sock = paths.control_socket();
        let listener = bind_control_socket(&sock).unwrap();
        tokio::spawn(listen_control(
            listener,
            sock,
            shutdown.clone(),
            Control {
                status_rx: status_rx.clone(),
                event_tx: event_tx.clone(),
                command_tx,
                restart: Signal::new(),
            },
        ));
        let daemon = tokio::spawn(run(
            Config::default(),
            creds.clone(),
            paths.clone(),
            shutdown.clone(),
            status_tx,
            event_tx,
            command_rx,
        ));

        // A cast from the server needs the WebSocket, and so do socket commands:
        // `run_session` is what reads them.
        let api = Api::from_credentials(&creds).unwrap();
        let deadline = Instant::now() + SETTLE;
        let session_id = loop {
            if let Some(session_id) = remote_session(&api).await {
                break session_id;
            }
            assert!(
                Instant::now() < deadline,
                "the daemon's session never became remote-controllable within {SETTLE:?}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        };
        let mut live = Self {
            status_rx,
            shutdown,
            daemon,
            api,
            session_id,
            paths,
            _dir: dir,
        };
        // The server can list the session a moment before the daemon has it.
        live.wait_for("the daemon's own session", |status| status.server_connected)
            .await;
        live
    }

    async fn client(&self) -> Client {
        Client::connect(&self.paths, "integration test")
            .await
            .unwrap()
            .0
    }

    /// How phones and the web app cast: the server relays it over the WebSocket.
    async fn play_through_server(&self, item_id: &str) {
        reqwest::Client::new()
            .post(format!(
                "{}/Sessions/{}/Playing?PlayCommand=PlayNow&ItemIds={item_id}",
                self.api.server, self.session_id
            ))
            .header("Authorization", authorization(&self.api))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
    }

    async fn wait_for(
        &mut self,
        what: &str,
        done: impl FnMut(&PlayerStatus) -> bool,
    ) -> PlayerStatus {
        tokio::time::timeout(SETTLE, self.status_rx.wait_for(done))
            .await
            .unwrap_or_else(|_| panic!("{what} within {SETTLE:?}"))
            .unwrap()
            .clone()
    }

    async fn stop(self) {
        self.shutdown.fire();
        self.daemon.await.unwrap().unwrap();
    }
}

fn play_now(id: u64, item_id: &str) -> Request {
    Request {
        id,
        command: Command::Cast(CastEvent::PlayNow {
            item_ids: vec![item_id.into()],
            start_index: 0,
            start_ticks: None,
            audio_stream_index: None,
            subtitle_stream_index: None,
            media_source_id: None,
        }),
    }
}

fn is_play(event: &Event) -> bool {
    !matches!(event, Event::Reported { .. })
}

/// The next event `pick` accepts; statuses and every other event are skipped.
async fn next_event(client: &mut Client, pick: impl Fn(&Event) -> bool) -> Event {
    tokio::time::timeout(SETTLE, async {
        loop {
            let frame = client
                .next_frame()
                .await
                .unwrap()
                .expect("the daemon closed the connection");
            if let DaemonFrame::Event(event) = frame
                && pick(&event)
            {
                return event;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("no matching event within {SETTLE:?}"))
}

/// The whole cast path: capabilities, the WebSocket, the Play command arriving
/// over it, PlaybackInfo, and mpv fetching the stream with our auth.
#[tokio::test]
async fn a_cast_from_the_server_is_attributed_to_jellyfin() {
    let mut live = LiveDaemon::start().await;
    let episode = first_episode(&live.api).await;
    let mut client = live.client().await;
    live.play_through_server(&episode.id).await;

    let item_id = Some(episode.id.clone());
    assert_eq!(
        next_event(&mut client, is_play).await,
        Event::PlayLoading {
            item_id: item_id.clone(),
            origin: Origin::Jellyfin
        }
    );
    assert_eq!(
        next_event(&mut client, is_play).await,
        Event::PlayStarted {
            item_id,
            origin: Origin::Jellyfin
        }
    );
    live.wait_for("mpv playing S01E01", |status| {
        status.now_playing.as_ref().is_some_and(|now_playing| {
            now_playing.item_id == episode.id && now_playing.position_ticks > 0
        })
    })
    .await;
    live.stop().await;
}

#[tokio::test]
async fn a_play_over_the_socket_loads_and_starts_under_its_origin() {
    let live = LiveDaemon::start().await;
    let episode = first_episode(&live.api).await;
    let mut client = live.client().await;
    let origin = Origin::Client {
        client: client.id(),
        request: 8,
    };
    client.send(&play_now(8, &episode.id)).await.unwrap();

    let item_id = Some(episode.id.clone());
    assert_eq!(
        next_event(&mut client, is_play).await,
        Event::PlayLoading {
            item_id: item_id.clone(),
            origin
        }
    );
    assert_eq!(
        next_event(&mut client, is_play).await,
        Event::PlayStarted { item_id, origin }
    );
    live.stop().await;
}

/// The socket skips the server, which refuses an id it does not know.
#[tokio::test]
async fn a_play_the_daemon_cannot_prepare_fails_under_its_origin() {
    let live = LiveDaemon::start().await;
    let mut client = live.client().await;
    let origin = Origin::Client {
        client: client.id(),
        request: 1,
    };
    client
        .send(&play_now(1, "00000000000000000000000000000000"))
        .await
        .unwrap();

    assert!(matches!(
        next_event(&mut client, is_play).await,
        Event::PlayLoading { origin: loading, .. } if loading == origin
    ));
    assert!(matches!(
        next_event(&mut client, is_play).await,
        Event::PlayFailed { origin: failed, .. } if failed == origin
    ));
    assert!(live.status_rx.borrow().now_playing.is_none());
    live.stop().await;
}

/// jellytui reloads on this, so it must not arrive before the server has it.
#[tokio::test]
async fn a_stop_is_reported_once_the_server_has_it() {
    let live = LiveDaemon::start().await;
    let episode = first_episode(&live.api).await;
    let mut client = live.client().await;
    client.send(&play_now(1, &episode.id)).await.unwrap();
    next_event(&mut client, |event| {
        matches!(event, Event::PlayStarted { .. })
    })
    .await;

    let stop = Request {
        id: 2,
        command: Command::Cast(CastEvent::Stop),
    };
    client.send(&stop).await.unwrap();
    assert_eq!(
        next_event(&mut client, |event| matches!(
            event,
            Event::Reported {
                report: ReportKind::Stopped,
                ..
            }
        ))
        .await,
        Event::Reported {
            report: ReportKind::Stopped,
            item_id: episode.id.clone(),
            ok: true
        }
    );
    live.stop().await;
}
