use super::*;

#[test]
fn payload_is_direct_play() {
    let state = PlayingState {
        item_id: "i".into(),
        media_source_id: "m".into(),
        play_session_id: "p".into(),
        position_ticks: 10,
        is_paused: false,
        is_muted: false,
        volume: 80,
        audio_stream_index: 1,
        subtitle_stream_index: -1,
        can_seek: true,
        now_playing_queue: Arc::new(vec![json!({"Id": "i", "PlaylistItemId": "playlistItem0"})]),
    };
    let payload = state.to_json();
    assert_eq!(payload["PlayMethod"], "DirectPlay");
    assert_eq!(payload["ItemId"], "i");
    assert_eq!(payload["VolumeLevel"], 80);
    assert_eq!(
        payload["NowPlayingQueue"][0]["PlaylistItemId"],
        "playlistItem0"
    );
}
