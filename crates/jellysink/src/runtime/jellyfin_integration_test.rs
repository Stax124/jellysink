//! The daemon against a live Jellyfin: the one `testserver/start.sh` starts.
//! Headless and `--no-config` mpv, as in `mpv/integration_test.rs`.

use super::run;
use crate::daemon::signal::Signal;
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

/// The whole cast path: capabilities, the WebSocket, the Play command arriving
/// over it, PlaybackInfo, and mpv fetching the stream with our auth.
#[tokio::test]
async fn a_play_command_from_jellytui_plays_in_mpv() {
    let creds = test_credentials().await;
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::from_override(Some(dir.path().to_path_buf())).unwrap();
    Field::MpvArgs
        .write(&paths, "--no-config --vo=null --ao=null --really-quiet")
        .unwrap();
    let shutdown = Signal::new();
    let (status_tx, mut status_rx) = watch::channel(PlayerStatus::idle(
        creds.server.clone(),
        creds.username.clone(),
    ));
    let (_ext_tx, ext_rx) = mpsc::unbounded_channel();
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
    let episode = first_episode(&jellytui).await;
    let deadline = Instant::now() + SETTLE;
    while !remote_controllable(&jellytui).await {
        assert!(
            Instant::now() < deadline,
            "the daemon's session never became remote-controllable within {SETTLE:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let session = jellytui
        .session_for_device()
        .await
        .unwrap()
        .expect("the daemon's session");
    jellytui
        .play_now(&session.id, &episode.id, 0)
        .await
        .unwrap();

    tokio::time::timeout(
        SETTLE,
        status_rx.wait_for(|status| {
            status.now_playing.as_ref().is_some_and(|now_playing| {
                now_playing.item_id == episode.id && now_playing.position_ticks > 0
            })
        }),
    )
    .await
    .expect("mpv never reported playing S01E01")
    .unwrap();

    shutdown.fire();
    daemon.await.unwrap().unwrap();
}
