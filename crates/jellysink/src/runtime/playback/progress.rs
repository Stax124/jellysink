//! Sampling mpv and reporting where playback has got to, to Jellyfin and to
//! the status snapshot core publishes over `stop.sock`.

use crate::report::{PlayingState, Report};
use crate::runtime::state::Runtime;
use jellysink_core::status::{NowPlaying, PlayerStatus};

impl Runtime {
    pub(in crate::runtime) async fn tick_progress(&mut self) {
        if self.mpv.is_none() || self.current.is_none() {
            return;
        }
        self.sample_mpv_state().await;
        self.send_progress();
    }

    /// Re-announces the current play on a freshly connected session. Resampled
    /// first, since nothing polled mpv while the socket was down.
    pub(in crate::runtime) async fn reannounce(&mut self) {
        if self.mpv.is_none() || self.current.is_none() {
            return;
        }
        self.sample_mpv_state().await;
        self.send_start();
    }

    /// Keeps the previous value per failed read: a half-dead IPC socket must
    /// not rewrite state.
    async fn sample_mpv_state(&mut self) {
        self.sample_position().await;
        let Some(mpv) = self.mpv.as_mut() else {
            return;
        };
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

    fn snapshot(&self) -> Option<PlayingState> {
        let prepared = self.current.as_ref()?;
        Some(PlayingState {
            item_id: prepared.item_id.clone(),
            media_source_id: prepared.media_source_id.clone(),
            play_session_id: prepared.play_session_id.clone(),
            position_ticks: self.last_ticks,
            is_paused: self.paused,
            is_muted: self.muted,
            volume: self.volume,
            audio_stream_index: prepared.audio_stream_index.unwrap_or(-1),
            subtitle_stream_index: prepared.subtitle_stream_index.unwrap_or(-1),
            queue: self.window.items().to_vec(),
        })
    }

    pub(super) fn send_start(&self) {
        if let Some(s) = self.snapshot() {
            let _ = self.report_tx.send(Report::Start(s));
        }
        self.publish_status();
    }

    pub(in crate::runtime) fn send_progress(&self) {
        if let Some(s) = self.snapshot() {
            let _ = self.report_tx.send(Report::Progress(s));
        }
        self.publish_status();
    }

    pub(super) fn send_stopped(&self) {
        if let Some(mut s) = self.snapshot() {
            s.is_paused = true;
            let _ = self.report_tx.send(Report::Stopped(s));
        }
    }

    pub(super) fn publish_status(&self) {
        let now_playing = self.current.as_ref().map(|prepared| NowPlaying {
            item_id: prepared.item_id.clone(),
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
            art_url: jellysink_core::jellyfin::url::image_url(&self.api.server, &prepared.item_id),
        });
        self.status_tx.send_replace(PlayerStatus {
            server: self.api.server.clone(),
            username: self.username.clone(),
            now_playing,
        });
    }
}
