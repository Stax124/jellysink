//! A snapshot of what the daemon is doing right now: the wire format of
//! `jellysink status` and of jellytui's once-a-second footer poll.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerStatus {
    pub server: String,
    pub username: String,
    pub now_playing: Option<NowPlaying>,
}

impl PlayerStatus {
    /// The value published before playback ever starts, or once it stops.
    pub fn idle(server: String, username: String) -> Self {
        Self {
            server,
            username,
            now_playing: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NowPlaying {
    pub item_id: String,
    pub title: String,
    pub position_ticks: i64,
    pub is_paused: bool,
    pub is_muted: bool,
    pub volume: i64,
    pub has_next: bool,
    pub has_previous: bool,
    pub queue_index: usize,
    pub queue_len: usize,
    /// The item's primary image, carrying the access token in the query
    /// string — see [`crate::jellyfin::url::image_url`].
    pub art_url: String,
}

#[cfg(test)]
#[path = "status_test.rs"]
mod tests;
