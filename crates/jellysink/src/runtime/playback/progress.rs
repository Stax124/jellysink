//! Sampling mpv and reporting where playback has got to.

use crate::report::{PlayingState, Report};
use crate::runtime::state::Runtime;

impl Runtime {
    pub(in crate::runtime) async fn tick_progress(&mut self) {
        if self.mpv.is_none() || self.current.is_none() {
            return;
        }
        self.sample_mpv_state().await;
        self.send_progress();
    }

    /// Re-announces the current play on a freshly connected session: a server
    /// that dropped the session needs a start to show a now-playing again.
    /// Resampled first, since nothing polled mpv while the socket was down.
    pub(in crate::runtime) async fn reannounce(&mut self) {
        if self.mpv.is_none() || self.current.is_none() {
            return;
        }
        self.sample_mpv_state().await;
        self.send_start();
    }

    /// Pulls position, pause, volume and mute out of mpv, keeping the previous
    /// value per failed read: a half-dead IPC socket must not rewrite state.
    async fn sample_mpv_state(&mut self) {
        let Some(mpv) = self.mpv.as_mut() else {
            return;
        };
        // A dead/zero sample during unload must not lose a known position.
        let live = mpv.time_pos().await.ok();
        self.last_ticks = jellysink_core::ticks::coalesce_position_ticks(live, self.last_ticks);
        if let Ok(p) = mpv.paused().await {
            self.paused = p;
        }
        if let Ok(v) = mpv.volume().await {
            self.volume = v;
        }
        if let Ok(m) = mpv.muted().await {
            self.muted = m;
        }
    }

    fn snapshot(&self, position_ticks: i64) -> Option<PlayingState> {
        let prep = self.current.as_ref()?;
        let item_id = self.item_id.as_ref()?;
        Some(PlayingState {
            item_id: item_id.clone(),
            media_source_id: prep.media_source_id.clone(),
            play_session_id: prep.play_session_id.clone(),
            position_ticks,
            is_paused: self.paused,
            is_muted: self.muted,
            volume: self.volume,
            audio_stream_index: prep.audio_stream_index.unwrap_or(-1),
            subtitle_stream_index: prep.subtitle_stream_index.unwrap_or(-1),
            can_seek: true,
            now_playing_queue: self.window.now_playing_queue(),
        })
    }

    pub(super) fn send_start(&self) {
        if let Some(s) = self.snapshot(self.last_ticks) {
            let _ = self.report_tx.send(Report::Start(s));
        }
        self.publish_status();
    }

    pub(in crate::runtime) fn send_progress(&self) {
        if let Some(s) = self.snapshot(self.last_ticks) {
            let _ = self.report_tx.send(Report::Progress(s));
        }
        self.publish_status();
    }

    pub(super) fn send_stopped(&self) {
        if let Some(mut s) = self.snapshot(self.last_ticks) {
            s.is_paused = true;
            let _ = self.report_tx.send(Report::Stopped(s));
        }
    }
}
