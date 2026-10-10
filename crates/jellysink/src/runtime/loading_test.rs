use crate::runtime::state::Runtime;
use color_eyre::eyre::eyre;
use jellysink_core::config::{Config, Credentials, Paths};
use jellysink_core::control::{Event, Origin};
use jellysink_core::jellyfin::auth::Api;
use jellysink_core::status::PlayerStatus;
use tempfile::TempDir;
use tokio::sync::{broadcast, mpsc, watch};

struct Offline {
    rt: Runtime,
    status_rx: watch::Receiver<PlayerStatus>,
    events: broadcast::Receiver<Event>,
    _dir: TempDir,
}

impl Offline {
    fn new() -> Self {
        jellysink_core::install_crypto_provider();
        let creds = Credentials {
            server: "http://jelly.example".into(),
            username: "admin".into(),
            user_id: "u1".into(),
            access_token: "secret-token".into(),
            device_id: "d1".into(),
        };
        let dir = TempDir::new().unwrap();
        let paths = Paths::from_override(Some(dir.path().to_path_buf())).unwrap();
        let (status_tx, status_rx) = watch::channel(PlayerStatus::idle(
            creds.server.clone(),
            creds.username.clone(),
        ));
        let (event_tx, events) = broadcast::channel(16);
        let (report_tx, _) = mpsc::unbounded_channel();
        let rt = Runtime::new(
            Api::from_credentials(&creds).unwrap(),
            Config::default(),
            paths,
            report_tx,
            creds.username,
            status_tx,
            event_tx,
        );
        Self {
            rt,
            status_rx,
            events,
            _dir: dir,
        }
    }

    fn emitted(&mut self) -> Vec<Event> {
        std::iter::from_fn(|| self.events.try_recv().ok()).collect()
    }
}

#[test]
fn every_load_ends_in_exactly_one_outcome() {
    let mut offline = Offline::new();
    let first = Origin::Client {
        client: 1,
        request: 8,
    };
    offline.rt.begin_load(Some("a".into()), first);
    assert!(offline.status_rx.borrow().loading);
    offline.rt.begin_load(Some("b".into()), Origin::Mpris);
    offline.rt.cancel_load(Origin::Daemon);
    offline.rt.cancel_load(Origin::Daemon);
    offline.rt.fail_load(&eyre!("nothing is loading"));
    assert!(!offline.status_rx.borrow().loading);

    assert_eq!(
        offline.emitted(),
        [
            Event::PlayLoading {
                item_id: Some("a".into()),
                origin: first
            },
            Event::PlayCancelled {
                item_id: Some("a".into()),
                origin: first,
                by: Origin::Mpris
            },
            Event::PlayLoading {
                item_id: Some("b".into()),
                origin: Origin::Mpris
            },
            Event::PlayCancelled {
                item_id: Some("b".into()),
                origin: Origin::Mpris,
                by: Origin::Daemon
            },
        ]
    );
}

#[test]
fn a_failed_load_does_not_carry_the_token_out() {
    let mut offline = Offline::new();
    offline.rt.begin_load(None, Origin::Jellyfin);
    offline.rt.fail_load(&eyre!(
        "loadfile http://jelly.example/Videos/a/stream?static=true&ApiKey=secret-token"
    ));
    let Some(Event::PlayFailed { error, .. }) = offline.emitted().pop() else {
        panic!("no play_failed");
    };
    assert!(!error.contains("secret-token"), "{error}");
}
