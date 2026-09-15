use super::*;
use jellysink_core::status::NowPlaying;
use jellysink_core::status::PlayerStatus;

pub(super) fn playing_status(paused: bool, has_next: bool, has_previous: bool) -> PlayerStatus {
    PlayerStatus {
        server: "http://x".into(),
        username: "admin".into(),
        now_playing: Some(NowPlaying {
            item_id: "item-1".into(),
            title: "Ep 1".into(),
            position_ticks: 1_200_000_000,
            run_time_ticks: Some(14_220_809_999),
            is_paused: paused,
            is_muted: false,
            volume: 80,
            has_next,
            has_previous,
            queue_index: 2,
            queue_len: 5,
            art_url: "http://x/Items/item-1/Images/Primary?ApiKey=tok".into(),
        }),
    }
}

fn idle_status() -> PlayerStatus {
    PlayerStatus::idle("http://x".into(), "admin".into())
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
    let art = <&str>::try_from(meta.get("mpris:artUrl").unwrap()).unwrap();
    assert_eq!(art, "http://x/Items/item-1/Images/Primary?ApiKey=tok");
}

/// No length, no seek bar in any desktop widget.
#[test]
fn metadata_carries_the_length_in_microseconds_and_omits_it_when_unknown() {
    let (iface, _rx) = player(playing_status(false, true, true));
    let length = i64::try_from(iface.metadata().get("mpris:length").unwrap()).unwrap();
    assert_eq!(length, 1_422_080_999);

    let mut status = playing_status(false, true, true);
    if let Some(now_playing) = status.now_playing.as_mut() {
        now_playing.run_time_ticks = None;
    }
    let (iface, _rx) = player(status);
    assert!(!iface.metadata().contains_key("mpris:length"));
}

#[test]
fn a_seek_is_signalled_but_ordinary_playback_progress_is_not() {
    let (before, mut after) = (
        playing_status(false, true, true),
        playing_status(false, true, true),
    );

    if let Some(now_playing) = after.now_playing.as_mut() {
        now_playing.position_ticks += 10_000_000; // one second of playback
    }
    assert_eq!(seeked_position(&before, &after), None);

    if let Some(now_playing) = after.now_playing.as_mut() {
        now_playing.position_ticks = 1_800_000_000;
    }
    assert_eq!(seeked_position(&before, &after), Some(180_000_000));

    if let Some(now_playing) = after.now_playing.as_mut() {
        now_playing.position_ticks = 600_000_000;
    }
    assert_eq!(seeked_position(&before, &after), Some(60_000_000));
}

/// The next episode starts at 0, which is backwards but not a seek — the
/// metadata change is what re-bases the bar.
#[test]
fn a_new_item_is_not_a_seek() {
    let before = playing_status(false, true, true);
    let mut after = playing_status(false, true, true);
    if let Some(now_playing) = after.now_playing.as_mut() {
        now_playing.item_id = "item-2".into();
        now_playing.position_ticks = 0;
    }
    assert_eq!(seeked_position(&before, &after), None);
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
