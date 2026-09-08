//! A snapshot of what the daemon is doing right now, for `jellysink status`.

use super::state::Runtime;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PlayerStatus {
    pub(crate) server: String,
    pub(crate) username: String,
    pub(crate) now_playing: Option<NowPlaying>,
}

impl PlayerStatus {
    /// The value published before playback ever starts, or once it stops.
    pub(crate) fn idle(server: String, username: String) -> Self {
        Self {
            server,
            username,
            now_playing: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct NowPlaying {
    pub(crate) item_id: String,
    pub(crate) title: String,
    pub(crate) position_ticks: i64,
    pub(crate) is_paused: bool,
    pub(crate) is_muted: bool,
    pub(crate) volume: i64,
    pub(crate) has_next: bool,
    pub(crate) has_previous: bool,
    pub(crate) queue_index: usize,
    pub(crate) queue_len: usize,
    /// The item's primary image, carrying the access token in the query
    /// string — see [`crate::jellyfin::url::image_url`].
    pub(crate) art_url: String,
}

impl Runtime {
    fn build_status(&self) -> PlayerStatus {
        let now_playing =
            self.current
                .as_ref()
                .zip(self.item_id.as_ref())
                .map(|(prep, item_id)| NowPlaying {
                    item_id: item_id.clone(),
                    title: prep.title.clone(),
                    position_ticks: self.last_ticks,
                    is_paused: self.paused,
                    is_muted: self.muted,
                    volume: self.volume,
                    has_next: self.window.has_next(),
                    has_previous: self.window.index() > 0,
                    queue_index: self.window.index(),
                    queue_len: self.window.len(),
                    art_url: crate::jellyfin::url::image_url(
                        &self.api.server,
                        item_id,
                        &self.api.token,
                    ),
                });
        PlayerStatus {
            server: self.api.server.clone(),
            username: self.username.clone(),
            now_playing,
        }
    }

    /// Pushes the current state onto the status watch channel. Called at the
    /// same points that already report to Jellyfin (`send_start`,
    /// `send_progress`) plus the end of `stop_playback`, so `jellysink status`
    /// never needs to poll `Runtime` directly.
    pub(super) fn publish_status(&self) {
        self.status_tx.send_replace(self.build_status());
    }
}

#[cfg(test)]
#[path = "status_test.rs"]
mod tests;
