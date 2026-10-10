use super::*;

/// The daemon serializes, `jellysink status --json` and jellytui's footer
/// deserialize; a renamed field is a silently empty footer and a broken script.
#[test]
fn status_serializes_the_field_names_its_readers_expect() {
    let status = PlayerStatus {
        server: "http://x".into(),
        username: "admin".into(),
        now_playing: Some(NowPlaying {
            item_id: "1".into(),
            title: "Ep 1".into(),
            position_ticks: 12345,
            run_time_ticks: Some(14_220_809_999),
            is_paused: false,
            is_muted: false,
            volume: 80,
            has_next: true,
            has_previous: false,
            queue_index: 0,
            queue_len: 5,
            art_url: "http://x/Items/1/Images/Primary".into(),
        }),
        loading: false,
        plays_started: 0,
    };

    let json = serde_json::to_value(&status).unwrap();
    assert_eq!(
        field_names(&json),
        [
            "loading",
            "now_playing",
            "plays_started",
            "server",
            "username"
        ]
    );
    assert_eq!(
        field_names(&json["now_playing"]),
        [
            "art_url",
            "has_next",
            "has_previous",
            "is_muted",
            "is_paused",
            "item_id",
            "position_ticks",
            "queue_index",
            "queue_len",
            "run_time_ticks",
            "title",
            "volume",
        ]
    );
}

/// jellytui can be newer than the daemon process it polls.
#[test]
fn a_status_from_a_daemon_without_play_tracking_parses_as_idle() {
    let status: PlayerStatus = serde_json::from_value(serde_json::json!({
        "server": "http://x", "username": "admin", "now_playing": null
    }))
    .unwrap();
    assert!(!status.loading);
    assert_eq!(status.plays_started, 0);
}

/// Alphabetical: `serde_json`'s map is sorted, and field order is not part of
/// what a reader depends on.
fn field_names(value: &serde_json::Value) -> Vec<&str> {
    value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect()
}
