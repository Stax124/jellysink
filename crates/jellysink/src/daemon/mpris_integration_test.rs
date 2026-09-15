use super::tests::playing_status;
use super::*;

const NO_BUS: &str = "no D-Bus session bus reachable: set DBUS_SESSION_BUS_ADDRESS, \
or run the suite under `dbus-run-session -- cargo test`";

/// A private, per-process well-known name: claiming the real `BUS_NAME` bumps a
/// running jellysink off MPRIS, and this test bypasses `InstanceLock`.
fn test_bus_name() -> String {
    format!(
        "org.mpris.MediaPlayer2.jellysink.selftest.p{}",
        std::process::id()
    )
}

/// Needs a session bus: CI runs the suite under `dbus-run-session`, which
/// provides a private one that dies with the job. See `.github/workflows/ci.yml`.
#[tokio::test]
async fn live_smoke_test_against_the_real_session_bus() {
    let bus_name = test_bus_name();
    let (_status_tx, status_rx) = watch::channel(playing_status(false, true, false));
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel();
    let shutdown = Signal::new();
    let shared = Arc::new(Shared {
        status_rx,
        cmd_tx,
        shutdown: shutdown.clone(),
    });
    let _connection = zbus::connection::Builder::session()
        .expect(NO_BUS)
        .serve_at(OBJECT_PATH, RootIface(shared.clone()))
        .unwrap()
        .serve_at(OBJECT_PATH, PlayerIface(shared))
        .unwrap()
        .name(bus_name.as_str())
        .unwrap()
        .build()
        .await
        .expect(NO_BUS);

    let conn = zbus::Connection::session().await.expect(NO_BUS);
    let reply = conn
        .call_method(
            Some(bus_name.as_str()),
            OBJECT_PATH,
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &("org.mpris.MediaPlayer2.Player", "PlaybackStatus"),
        )
        .await
        .unwrap();
    let status: zbus::zvariant::OwnedValue = reply.body().deserialize().unwrap();
    eprintln!("PlaybackStatus = {status:?}");
    assert_eq!(String::try_from(status).unwrap(), "Playing");

    // A widget reads the length off the wire as an int64; any other signature
    // leaves it with no seek bar and no error to explain why.
    let reply = conn
        .call_method(
            Some(bus_name.as_str()),
            OBJECT_PATH,
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &("org.mpris.MediaPlayer2.Player", "Metadata"),
        )
        .await
        .unwrap();
    let metadata: OwnedValue = reply.body().deserialize().unwrap();
    let metadata = HashMap::<String, OwnedValue>::try_from(metadata).unwrap();
    let length = metadata.get("mpris:length").expect("mpris:length");
    assert_eq!(length.value_signature().to_string(), "x", "{length:?}");
    assert_eq!(i64::try_from(length).unwrap(), 1_422_080_999);

    conn.call_method(
        Some(bus_name.as_str()),
        OBJECT_PATH,
        Some("org.mpris.MediaPlayer2.Player"),
        "Next",
        &(),
    )
    .await
    .unwrap();
    assert_eq!(cmd_rx.recv().await.unwrap(), CastEvent::Next);

    conn.call_method(
        Some(bus_name.as_str()),
        OBJECT_PATH,
        Some("org.mpris.MediaPlayer2"),
        "Quit",
        &(),
    )
    .await
    .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), shutdown.fired())
        .await
        .expect("Quit should fire the shutdown signal");
}
