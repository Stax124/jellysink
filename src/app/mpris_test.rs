use super::*;
use crate::runtime::PlayerStatus;
use crate::runtime::status::NowPlaying;

fn playing_status(paused: bool, has_next: bool, has_previous: bool) -> PlayerStatus {
    PlayerStatus {
        server: "http://x".into(),
        username: "tomas".into(),
        now_playing: Some(NowPlaying {
            item_id: "item-1".into(),
            title: "Ep 1".into(),
            position_ticks: 1_200_000_000,
            is_paused: paused,
            is_muted: false,
            volume: 80,
            has_next,
            has_previous,
            queue_index: 2,
            queue_len: 5,
        }),
    }
}

fn idle_status() -> PlayerStatus {
    PlayerStatus::idle("http://x".into(), "tomas".into())
}

fn player(status: PlayerStatus) -> (PlayerIface, mpsc::UnboundedReceiver<CastEvent>) {
    let (_status_tx, status_rx) = watch::channel(status);
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let shared = Arc::new(Shared {
        status_rx,
        cmd_tx,
        shutdown: Signal::new(),
    });
    (PlayerIface(shared), cmd_rx)
}

#[test]
fn track_id_sanitizes_hyphens_and_is_stable() {
    let id = track_id("ab12-cd34-ef56");
    assert_eq!(id.as_str(), "/org/jellysink/track/ab12_cd34_ef56");
    assert_eq!(id, track_id("ab12-cd34-ef56"));
    assert_ne!(id, track_id("other-item"));
}

#[test]
fn playback_status_reflects_playing_paused_and_stopped() {
    let (iface, _rx) = player(playing_status(false, true, true));
    assert_eq!(iface.playback_status(), "Playing");

    let (iface, _rx) = player(playing_status(true, true, true));
    assert_eq!(iface.playback_status(), "Paused");

    let (iface, _rx) = player(idle_status());
    assert_eq!(iface.playback_status(), "Stopped");
}

#[test]
fn metadata_is_empty_when_idle_and_carries_the_title_when_playing() {
    let (iface, _rx) = player(idle_status());
    assert!(iface.metadata().is_empty());

    let (iface, _rx) = player(playing_status(false, true, true));
    let meta = iface.metadata();
    assert!(meta.contains_key("mpris:trackid"));
    let title = <&str>::try_from(meta.get("xesam:title").unwrap()).unwrap();
    assert_eq!(title, "Ep 1");
}

#[test]
fn can_go_next_and_previous_follow_the_queue_flags() {
    let (iface, _rx) = player(playing_status(false, true, false));
    assert!(iface.can_go_next());
    assert!(!iface.can_go_previous());

    let (iface, _rx) = player(playing_status(false, false, true));
    assert!(!iface.can_go_next());
    assert!(iface.can_go_previous());
}

#[test]
fn can_play_pause_seek_are_false_when_idle() {
    let (iface, _rx) = player(idle_status());
    assert!(!iface.can_play());
    assert!(!iface.can_pause());
    assert!(!iface.can_seek());
}

#[test]
fn volume_and_position_convert_units() {
    let (iface, _rx) = player(playing_status(false, true, true));
    assert_eq!(iface.volume(), 0.8);
    assert_eq!(iface.position(), 120_000_000); // ticks / 10 = microseconds
}

#[test]
fn play_pause_stop_next_previous_forward_the_matching_cast_event() {
    let (iface, mut rx) = player(playing_status(false, true, true));
    iface.play();
    assert_eq!(rx.try_recv().unwrap(), CastEvent::Unpause);
    iface.pause();
    assert_eq!(rx.try_recv().unwrap(), CastEvent::Pause);
    iface.play_pause();
    assert_eq!(rx.try_recv().unwrap(), CastEvent::PlayPause);
    iface.stop();
    assert_eq!(rx.try_recv().unwrap(), CastEvent::Stop);
    iface.next();
    assert_eq!(rx.try_recv().unwrap(), CastEvent::Next);
    iface.previous();
    assert_eq!(rx.try_recv().unwrap(), CastEvent::Previous);
}

#[test]
fn set_volume_converts_the_0_to_1_range_to_a_percentage() {
    let (iface, mut rx) = player(playing_status(false, true, true));
    iface.set_volume(0.55);
    assert_eq!(rx.try_recv().unwrap(), CastEvent::SetVolume { volume: 55 });
}

#[test]
fn seek_adds_a_relative_microsecond_offset_to_the_absolute_position() {
    let (iface, mut rx) = player(playing_status(false, true, true));
    // position_ticks = 1_200_000_000 (120s); seek back 10s = -10_000_000us.
    iface.seek(-10_000_000);
    assert_eq!(
        rx.try_recv().unwrap(),
        CastEvent::Seek {
            ticks: 1_100_000_000
        }
    );
}

#[test]
fn seek_never_goes_negative() {
    let (iface, mut rx) = player(playing_status(false, true, true));
    iface.seek(-1_000_000_000_000);
    assert_eq!(rx.try_recv().unwrap(), CastEvent::Seek { ticks: 0 });
}

#[test]
fn set_position_is_a_no_op_for_a_track_id_that_is_not_the_current_one() {
    let (iface, mut rx) = player(playing_status(false, true, true));
    iface.set_position(track_id("some-other-item"), 5_000_000);
    assert!(rx.try_recv().is_err());
}

#[test]
fn set_position_seeks_when_the_track_id_matches_the_current_item() {
    let (iface, mut rx) = player(playing_status(false, true, true));
    iface.set_position(track_id("item-1"), 5_000_000);
    assert_eq!(
        rx.try_recv().unwrap(),
        CastEvent::Seek { ticks: 50_000_000 }
    );
}

#[test]
fn quit_fires_the_shutdown_signal() {
    let (_status_tx, status_rx) = watch::channel(idle_status());
    let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel();
    let shutdown = Signal::new();
    let iface = RootIface(Arc::new(Shared {
        status_rx,
        cmd_tx,
        shutdown: shutdown.clone(),
    }));
    iface.quit();
    assert!(shutdown.take());
}

#[tokio::test]
#[ignore = "manual smoke test: registers a real D-Bus session-bus service"]
async fn live_smoke_test_against_the_real_session_bus() {
    let (_status_tx, status_rx) = watch::channel(playing_status(false, true, false));
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel();
    let shutdown = Signal::new();
    super::start(status_rx, cmd_tx, shutdown.clone()).await;

    let conn = zbus::Connection::session().await.unwrap();
    let reply = conn
        .call_method(
            Some("org.mpris.MediaPlayer2.jellysink"),
            "/org/mpris/MediaPlayer2",
            Some("org.freedesktop.DBus.Properties"),
            "Get",
            &("org.mpris.MediaPlayer2.Player", "PlaybackStatus"),
        )
        .await
        .unwrap();
    let status: zbus::zvariant::OwnedValue = reply.body().deserialize().unwrap();
    eprintln!("PlaybackStatus = {status:?}");
    assert_eq!(String::try_from(status).unwrap(), "Playing");

    conn.call_method(
        Some("org.mpris.MediaPlayer2.jellysink"),
        "/org/mpris/MediaPlayer2",
        Some("org.mpris.MediaPlayer2.Player"),
        "Next",
        &(),
    )
    .await
    .unwrap();
    assert_eq!(cmd_rx.recv().await.unwrap(), CastEvent::Next);

    conn.call_method(
        Some("org.mpris.MediaPlayer2.jellysink"),
        "/org/mpris/MediaPlayer2",
        Some("org.mpris.MediaPlayer2"),
        "Quit",
        &(),
    )
    .await
    .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(shutdown.take());
}
