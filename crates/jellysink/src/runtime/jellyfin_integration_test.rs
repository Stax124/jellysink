//! The daemon against a live Jellyfin: the one `testserver/start.sh` starts.
//! Headless and `--no-config` mpv, as in `mpv/integration_test.rs`.

use super::run;
use crate::daemon::signal::Signal;
use jellysink_core::cast::CastEvent;
use jellysink_core::config::{Config, Credentials, Field, Paths};
use jellysink_core::jellyfin::auth::{Api, login};
use jellysink_core::jellyfin::browse::ItemQuery;
use jellysink_core::jellyfin::model::{Item, ItemList};
use jellysink_core::jellyfin::session::websocket_url;
use jellysink_core::status::PlayerStatus;
use serde::Deserialize;
use serde_json::Value;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};

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

/// Whether the server has a live WebSocket for this device: a login alone
/// already lists the session, just without remote control.
async fn remote_controllable(api: &Api) -> bool {
    let header = api.mpv_auth_header_field();
    let authorization = header.strip_prefix("Authorization: ").unwrap();
    let sessions: Vec<Value> = reqwest::Client::new()
        .get(format!(
            "{}/Sessions?deviceId={}",
            api.server, api.device_id
        ))
        .header("Authorization", authorization)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    sessions
        .iter()
        .any(|session| session["SupportsRemoteControl"] == true)
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

/// `run` against the test server, and the session jellytui would address.
struct LiveDaemon {
    status_rx: watch::Receiver<PlayerStatus>,
    /// The MPRIS channel: a cast here skips the server, which refuses unknown ids.
    ext_tx: mpsc::UnboundedSender<CastEvent>,
    shutdown: Signal,
    daemon: tokio::task::JoinHandle<color_eyre::Result<()>>,
    jellytui: Api,
    session_id: String,
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
        let (ext_tx, ext_rx) = mpsc::unbounded_channel();
        let daemon = tokio::spawn(run(
            Config::default(),
            creds.clone(),
            paths,
            shutdown.clone(),
            status_tx,
            ext_rx,
        ));

        // jellytui shares the daemon's cred.json, and so its device id.
        let jellytui = Api::from_credentials(&creds).unwrap();
        let deadline = Instant::now() + SETTLE;
        while !remote_controllable(&jellytui).await {
            assert!(
                Instant::now() < deadline,
                "the daemon's session never became remote-controllable within {SETTLE:?}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let session_id = jellytui
            .session_for_device()
            .await
            .unwrap()
            .expect("the daemon's session")
            .id;
        Self {
            status_rx,
            ext_tx,
            shutdown,
            daemon,
            jellytui,
            session_id,
            _dir: dir,
        }
    }

    async fn play(&self, item_id: &str) {
        self.jellytui
            .play_now(&self.session_id, item_id, 0)
            .await
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

/// The whole cast path: capabilities, the WebSocket, the Play command arriving
/// over it, PlaybackInfo, and mpv fetching the stream with our auth.
#[tokio::test]
async fn a_play_command_from_jellytui_plays_in_mpv() {
    let mut live = LiveDaemon::start().await;
    let episode = first_episode(&live.jellytui).await;
    live.play(&episode.id).await;

    live.wait_for("mpv reporting S01E01 playing and loaded", |status| {
        status.plays_started == 1
            && !status.loading
            && status.now_playing.as_ref().is_some_and(|now_playing| {
                now_playing.item_id == episode.id && now_playing.position_ticks > 0
            })
    })
    .await;
    live.stop().await;
}

/// jellytui shows a play as starting until `loading` drops, so a failure has to
/// drop it as surely as mpv loading the file does.
#[tokio::test]
async fn a_play_the_daemon_cannot_prepare_ends_its_loading() {
    let mut live = LiveDaemon::start().await;
    live.ext_tx
        .send(CastEvent::PlayNow {
            item_ids: vec!["00000000000000000000000000000000".into()],
            start_index: 0,
            start_ticks: None,
            audio_stream_index: None,
            subtitle_stream_index: None,
            media_source_id: None,
        })
        .unwrap();

    let status = live
        .wait_for("the failed play settling", |status| {
            status.plays_started == 1 && !status.loading
        })
        .await;
    assert!(status.now_playing.is_none(), "{status:?}");
    live.stop().await;
}
