use super::*;

#[test]
fn idle_status_has_no_now_playing() {
    let s = PlayerStatus::idle("http://x".into(), "tomas".into());
    assert!(s.now_playing.is_none());
    assert_eq!(s.username, "tomas");
}

/// The wire format between the daemon (serializes) and the CLI (deserializes)
/// over the stop socket — the part this feature actually adds.
#[test]
fn status_round_trips_through_json() {
    let s = PlayerStatus {
        server: "http://x".into(),
        username: "tomas".into(),
        now_playing: Some(NowPlaying {
            item_id: "1".into(),
            title: "Ep 1".into(),
            position_ticks: 12345,
            is_paused: false,
            is_muted: false,
            volume: 80,
            has_next: true,
            has_previous: false,
            queue_index: 0,
            queue_len: 5,
        }),
    };
    let json = serde_json::to_vec(&s).unwrap();
    let back: PlayerStatus = serde_json::from_slice(&json).unwrap();
    assert_eq!(back.username, "tomas");
    assert_eq!(back.now_playing.unwrap().title, "Ep 1");
}
