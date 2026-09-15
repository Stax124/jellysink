//! Building the status snapshot core publishes over `stop.sock`.

use super::state::Runtime;
use jellysink_core::status::{NowPlaying, PlayerStatus};

impl Runtime {
    fn build_status(&self) -> PlayerStatus {
        let now_playing =
            self.current
                .as_ref()
                .zip(self.item_id.as_ref())
                .map(|(prepared, item_id)| NowPlaying {
                    item_id: item_id.clone(),
                    title: prepared.title.clone(),
                    position_ticks: self.last_ticks,
                    run_time_ticks: prepared.run_time_ticks,
                    is_paused: self.paused,
                    is_muted: self.muted,
                    volume: self.volume,
                    has_next: self.window.has_next(),
                    has_previous: self.window.index() > 0,
                    queue_index: self.window.index(),
                    queue_len: self.window.len(),
                    art_url: jellysink_core::jellyfin::url::image_url(
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

    pub(super) fn publish_status(&self) {
        self.status_tx.send_replace(self.build_status());
    }
}
