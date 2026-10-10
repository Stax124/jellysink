use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerStatus {
    pub server: String,
    pub username: String,
    pub now_playing: Option<NowPlaying>,
    /// A play is under way and mpv has not loaded its file yet.
    pub loading: bool,
    /// The daemon has a live session with the server; without one it refuses
    /// commands from `control.sock`.
    pub server_connected: bool,
}

impl PlayerStatus {
    pub fn idle(server: String, username: String) -> Self {
        Self {
            server,
            username,
            now_playing: None,
            loading: false,
            server_connected: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NowPlaying {
    pub item_id: String,
    pub title: String,
    pub position_ticks: i64,
    pub run_time_ticks: Option<i64>,
    pub is_paused: bool,
    pub is_muted: bool,
    pub volume: i64,
    pub has_next: bool,
    pub has_previous: bool,
    pub queue_index: usize,
    pub queue_len: usize,
    pub art_url: String,
}

#[cfg(test)]
#[path = "status_test.rs"]
mod tests;
