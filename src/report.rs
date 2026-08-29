use serde_json::{Value, json};
use tokio::sync::mpsc;

/// Represents the current playing state of the media player
#[derive(Debug, Clone, PartialEq)]
pub struct PlayingState {
    pub item_id: String,
    pub media_source_id: String,
    pub play_session_id: String,
    pub position_ticks: i64,
    pub is_paused: bool,
    pub is_muted: bool,
    pub volume: i64,
    pub audio_stream_index: i64,
    pub subtitle_stream_index: i64,
    pub can_seek: bool,
    pub queue: Vec<String>,
}

impl PlayingState {
    /// Converts the playing state to something that we can send back to Jellyfin
    pub fn to_json(&self) -> Value {
        let now_playing_queue: Vec<Value> = self
            .queue
            .iter()
            .enumerate()
            .map(|(i, id)| {
                json!({
                    "Id": id,
                    "PlaylistItemId": format!("playlistItem{i}"),
                })
            })
            .collect();
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
            "NowPlayingQueue": now_playing_queue,
        })
    }
}

/// Represents a type of report to be sent back to Jellyfin
#[derive(Debug, Clone)]
pub enum Report {
    Start(PlayingState),
    Progress(PlayingState),
    Stopped(PlayingState),
}

/// Ordered, non-blocking session reports. Stopped then Start must never race.
pub fn spawn_reporter<F, Fut>(mut send: F) -> mpsc::UnboundedSender<Report>
where
    F: FnMut(Report) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send,
{
    let (tx, mut rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Some(report) = rx.recv().await {
            send(report).await;
        }
    });
    tx
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
            queue: vec!["i".into()],
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
        let tx = spawn_reporter(move |r| {
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
            queue: vec![],
        };
        tx.send(Report::Stopped(dummy.clone())).unwrap();
        tx.send(Report::Start(dummy)).unwrap();
        drop(tx);
        assert_eq!(seen_rx.recv().await, Some("stop"));
        assert_eq!(seen_rx.recv().await, Some("start"));
    }
}
