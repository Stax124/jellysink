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
fn the_transport_commands_other_clients_send_all_parse() {
    // These reach the daemon from the web app, a phone and MPRIS. jellytui
    // sends none of them: playback control is mpv's, see `specs/tui.md`.
    let playstate = |command: &str| CastEvent::from_ws("Playstate", &json!({"Command": command}));
    assert_eq!(playstate("Stop"), Some(CastEvent::Stop));
    assert_eq!(playstate("NextTrack"), Some(CastEvent::Next));
    assert_eq!(playstate("PreviousTrack"), Some(CastEvent::Previous));

    let general = |name: &str| CastEvent::from_ws("GeneralCommand", &json!({"Name": name}));
    assert_eq!(general("ToggleMute"), Some(CastEvent::ToggleMute));
    assert_eq!(
        general("ToggleFullscreen"),
        Some(CastEvent::ToggleFullscreen)
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

/// Control clients name commands by these; a renamed variant is a command
/// nobody parses any more.
#[test]
fn cast_commands_keep_their_wire_names() {
    let wire = |event: CastEvent| serde_json::to_value(event).unwrap();
    assert_eq!(
        wire(CastEvent::PlayNow {
            item_ids: vec!["e1".into()],
            start_index: 0,
            start_ticks: Some(5),
            audio_stream_index: None,
            subtitle_stream_index: Some(2),
            media_source_id: None,
        }),
        json!({
            "command": "play_now",
            "item_ids": ["e1"],
            "start_index": 0,
            "start_ticks": 5,
            "audio_stream_index": null,
            "subtitle_stream_index": 2,
            "media_source_id": null
        })
    );
    assert_eq!(
        wire(CastEvent::Seek { ticks: 10 }),
        json!({"command": "seek", "ticks": 10})
    );
    assert_eq!(
        wire(CastEvent::SetVolume { volume: 40 }),
        json!({"command": "set_volume", "volume": 40})
    );
    assert_eq!(
        wire(CastEvent::SetAudio { stream_index: 1 }),
        json!({"command": "set_audio", "stream_index": 1})
    );
    let tags: Vec<serde_json::Value> = [
        CastEvent::PlayNext { item_ids: vec![] },
        CastEvent::PlayLast { item_ids: vec![] },
        CastEvent::PlayPause,
        CastEvent::Pause,
        CastEvent::Unpause,
        CastEvent::Stop,
        CastEvent::Next,
        CastEvent::Previous,
        CastEvent::VolumeUp,
        CastEvent::VolumeDown,
        CastEvent::Mute,
        CastEvent::Unmute,
        CastEvent::ToggleMute,
        CastEvent::SetSubtitle { stream_index: 0 },
        CastEvent::ToggleFullscreen,
    ]
    .into_iter()
    .map(|event| wire(event)["command"].clone())
    .collect();
    assert_eq!(
        tags,
        [
            "play_next",
            "play_last",
            "play_pause",
            "pause",
            "unpause",
            "stop",
            "next",
            "previous",
            "volume_up",
            "volume_down",
            "mute",
            "unmute",
            "toggle_mute",
            "set_subtitle",
            "toggle_fullscreen",
        ]
    );
}
