use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Represents the current playing state of the media player
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PlayingState {
    pub(crate) item_id: String,
    pub(crate) media_source_id: String,
    pub(crate) play_session_id: String,
    pub(crate) position_ticks: i64,
    pub(crate) is_paused: bool,
    pub(crate) is_muted: bool,
    pub(crate) volume: i64,
    pub(crate) audio_stream_index: i64,
    pub(crate) subtitle_stream_index: i64,
    pub(crate) can_seek: bool,
    /// The prebuilt `NowPlayingQueue` payload. Shared rather than rebuilt per
    /// report — see `PlaylistWindow::now_playing`.
    pub(crate) now_playing_queue: Arc<Vec<Value>>,
}

impl PlayingState {
    /// Converts the playing state to something that we can send back to Jellyfin
    pub(crate) fn to_json(&self) -> Value {
        json!({
            "VolumeLevel": self.volume,
            "IsMuted": self.is_muted,
            "IsPaused": self.is_paused,
            "RepeatMode": "RepeatNone",
            "PositionTicks": self.position_ticks,
            "SubtitleStreamIndex": self.subtitle_stream_index,
            "AudioStreamIndex": self.audio_stream_index,
            "PlayMethod": "DirectPlay",
            "PlaySessionId": self.play_session_id,
            "MediaSourceId": self.media_source_id,
            "CanSeek": self.can_seek,
            "ItemId": self.item_id,
            "NowPlayingQueue": *self.now_playing_queue,
        })
    }
}

/// Represents a type of report to be sent back to Jellyfin
#[derive(Debug, Clone)]
pub(crate) enum Report {
    Start(PlayingState),
    Progress(PlayingState),
    Stopped(PlayingState),
}

/// Ordered, non-blocking session reports. Stopped then Start must never race.
///
/// The handle is returned rather than detached so a session owns its reporter
/// and can abort it on teardown.
pub(crate) fn spawn_reporter<F, Fut>(
    mut send: F,
) -> (mpsc::UnboundedSender<Report>, tokio::task::JoinHandle<()>)
where
    F: FnMut(Report) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send,
{
    let (tx, mut rx) = mpsc::unbounded_channel();
    let task = tokio::spawn(async move {
        while let Some(report) = rx.recv().await {
            send(report).await;
        }
    });
    (tx, task)
}

#[cfg(test)]
mod tests {
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
            now_playing_queue: Arc::new(vec![
                json!({"Id": "i", "PlaylistItemId": "playlistItem0"}),
            ]),
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
}
