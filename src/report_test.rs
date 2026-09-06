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
    let v = state.to_json();
    assert_eq!(v["PlayMethod"], "DirectPlay");
    assert_eq!(v["ItemId"], "i");
    assert_eq!(v["VolumeLevel"], 80);
    assert_eq!(v["NowPlayingQueue"][0]["PlaylistItemId"], "playlistItem0");
}

#[tokio::test]
async fn reports_are_fifo() {
    let (seen_tx, mut seen_rx) = mpsc::unbounded_channel();
    let (tx, _task) = spawn_reporter(move |r| {
        let seen_tx = seen_tx.clone();
        async move {
            seen_tx
                .send(match r {
                    Report::Start(_) => "start",
                    Report::Progress(_) => "progress",
                    Report::Stopped(_) => "stop",
                })
                .ok();
        }
    });
    let dummy = PlayingState {
        item_id: "i".into(),
        media_source_id: "m".into(),
        play_session_id: "p".into(),
        position_ticks: 0,
        is_paused: false,
        is_muted: false,
        volume: 100,
        audio_stream_index: -1,
        subtitle_stream_index: -1,
        can_seek: true,
        now_playing_queue: Arc::new(vec![]),
    };
    tx.send(Report::Stopped(dummy.clone())).unwrap();
    tx.send(Report::Start(dummy)).unwrap();
    drop(tx);
    assert_eq!(seen_rx.recv().await, Some("stop"));
    assert_eq!(seen_rx.recv().await, Some("start"));
}

#[tokio::test]
async fn the_reporter_task_ends_when_the_sender_is_dropped() {
    let (tx, task) = spawn_reporter(|_| async {});
    drop(tx);
    tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .expect("reporter should finish")
        .expect("reporter should not panic");
}
