use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::mpsc;

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
    /// The prebuilt `NowPlayingQueue` payload; see `PlaylistWindow::now_playing`.
    pub(crate) now_playing_queue: Arc<Vec<Value>>,
}

impl PlayingState {
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

#[derive(Debug, Clone)]
pub(crate) enum Report {
    Start(PlayingState),
    Progress(PlayingState),
    Stopped(PlayingState),
}

/// Ordered, non-blocking session reports: a Stopped must never overtake the
/// Start before it. The handle is returned so a session can abort it.
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
#[path = "report_test.rs"]
mod tests;
