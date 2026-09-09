use super::*;
use serde_json::json;

#[test]
fn play_now() {
    let ev = CastEvent::from_ws(
        "Play",
        &json!({
            "PlayCommand": "PlayNow",
            "ItemIds": ["a", "b"],
            "StartIndex": 1,
            "StartPositionTicks": 150000000,
            "AudioStreamIndex": 1,
            "SubtitleStreamIndex": 2
        }),
    )
    .unwrap();
    assert_eq!(
        ev,
        CastEvent::PlayNow {
            item_ids: vec!["a".into(), "b".into()],
            start_index: 1,
            start_ticks: Some(150_000_000),
            audio_stream_index: Some(1),
            subtitle_stream_index: Some(2),
            media_source_id: None,
        }
    );
}

#[test]
fn play_without_command_is_play_now() {
    let ev = CastEvent::from_ws("Play", &json!({"ItemIds": ["x"]})).unwrap();
    assert!(matches!(ev, CastEvent::PlayNow { .. }));
}

#[test]
fn play_next_and_last() {
    assert!(matches!(
        CastEvent::from_ws("Play", &json!({"PlayCommand":"PlayNext","ItemIds":["z"]})),
        Some(CastEvent::PlayNext { .. })
    ));
    assert!(matches!(
        CastEvent::from_ws("Play", &json!({"PlayCommand":"PlayLast","ItemIds":["z"]})),
        Some(CastEvent::PlayLast { .. })
    ));
}

#[test]
fn playstate_seek_and_pause() {
    assert_eq!(
        CastEvent::from_ws(
            "Playstate",
            &json!({"Command":"Seek","SeekPositionTicks": 10})
        ),
        Some(CastEvent::Seek { ticks: 10 })
    );
    assert_eq!(
        CastEvent::from_ws("Playstate", &json!({"Command":"PlayPause"})),
        Some(CastEvent::PlayPause)
    );
}

#[test]
fn general_volume_accepts_string() {
    let ev = CastEvent::from_ws(
        "GeneralCommand",
        &json!({"Name":"SetVolume","Arguments":{"Volume":"40"}}),
    )
    .unwrap();
    assert_eq!(ev, CastEvent::SetVolume { volume: 40 });
}

#[test]
fn navigation_commands_are_ignored() {
    assert_eq!(
        CastEvent::from_ws("GeneralCommand", &json!({"Name":"MoveUp"})),
        None
    );
}
