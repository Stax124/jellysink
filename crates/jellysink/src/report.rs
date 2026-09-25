use serde_json::{Value, json};

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
    /// Every queued item id, in order.
    pub(crate) queue: Vec<String>,
}

impl PlayingState {
    pub(crate) fn to_json(&self) -> Value {
        let now_playing_queue: Vec<Value> = self
            .queue
            .iter()
            .enumerate()
            .map(|(i, id)| json!({"Id": id, "PlaylistItemId": format!("playlistItem{i}")}))
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
            "CanSeek": true,
            "ItemId": self.item_id,
            "NowPlayingQueue": now_playing_queue,
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) enum Report {
    Start(PlayingState),
    Progress(PlayingState),
    Stopped(PlayingState),
}

impl Report {
    pub(crate) fn path(&self) -> &'static str {
        match self {
            Self::Start(_) => "/Sessions/Playing",
            Self::Progress(_) => "/Sessions/Playing/Progress",
            Self::Stopped(_) => "/Sessions/Playing/Stopped",
        }
    }

    pub(crate) fn state(&self) -> &PlayingState {
        match self {
            Self::Start(state) | Self::Progress(state) | Self::Stopped(state) => state,
        }
    }
}

#[cfg(test)]
#[path = "report_test.rs"]
mod tests;
