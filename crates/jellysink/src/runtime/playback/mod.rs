//! Applying a `CastEvent` to mpv. See `specs/playlist.md` and
//! `specs/tracks.md`.

mod load;
mod progress;
mod tracks;

use crate::media::PlayRequest;
use crate::mpv::SelectedTrack;
use crate::runtime::state::Runtime;
use color_eyre::eyre::eyre;
use serde_json::json;

impl Runtime {
    pub(in crate::runtime) async fn start_current(
        &mut self,
        req: &PlayRequest,
    ) -> color_eyre::Result<()> {
        let Some(item_id) = self.window.current().map(str::to_string) else {
            self.stop_playback(true).await;
            return Ok(());
        };

        self.prepared.clear();
        self.titles.clear();
        self.window.reset_to_current();

        let reuse = self.mpv.is_some();
        if reuse {
            self.sample_position().await;
            self.send_stopped();
            self.transitioning = true;
        }

        let (prepared, item) = match self.prepare_item(&item_id, req).await {
            Ok(pair) => pair,
            Err(e) => {
                tracing::error!("{e:#}");
                if reuse {
                    self.stop_playback(false).await;
                }
                return Ok(());
            }
        };

        if let Some(ref v) = item {
            self.maybe_expand_series(v, &item_id).await;
        } else {
            tracing::debug!(item = %item_id, "no item metadata; skipping series expand");
        }

        if let Err(e) = self.load_current(&prepared).await {
            tracing::error!("{e:#}");
            self.stop_playback(false).await;
            return Ok(());
        }

        tracing::info!(
            item = %prepared.item_id,
            title = %prepared.title,
            queue = self.window.len(),
            index = self.window.index(),
            "playing"
        );
        self.current = Some(prepared);
        self.paused = false;
        self.last_ticks = req.start_ticks.unwrap_or(0);
        self.external_subtitle_track_ids.clear();
        self.pending_start_ticks = req.start_ticks.filter(|ticks| *ticks > 0);

        self.send_start();
        // `loadfile ... replace` wiped mpv's playlist, so refill it around the
        // current file.
        self.fill_forward_into_mpv().await;
        self.fill_previous_into_mpv().await;
        Ok(())
    }

    /// A jump whose item cannot be prepared stops playback: nothing could
    /// describe what mpv is now playing.
    pub(in crate::runtime) async fn adopt_playlist_pos(&mut self) -> color_eyre::Result<()> {
        let playlist_pos = self
            .mpv
            .as_mut()
            .ok_or_else(|| eyre!("mpv missing"))?
            .playlist_pos()
            .await?;
        let Some(queue_index) = self.window.queue_index_at(playlist_pos) else {
            return Ok(());
        };
        let item_id = self.window.items()[queue_index].clone();
        if self
            .current
            .as_ref()
            .is_some_and(|current| current.item_id == item_id)
        {
            return Ok(());
        }
        // Via `prepare_item` (which caches plain requests itself) so remembered
        // tracks also reach a playlist jump and mpv's own autoplay.
        let prepared = match self.prepare_item(&item_id, &PlayRequest::default()).await {
            Ok((prepared, _)) => prepared,
            Err(e) => {
                self.stop_playback(true).await;
                return Err(e.wrap_err(format!("preparing {item_id} after a playlist jump")));
            }
        };
        self.send_stopped();
        self.window.adopt_index(queue_index);
        tracing::info!(item = %item_id, index = queue_index, "adopted mpv playlist jump");
        let title = prepared.title.clone();
        self.current = Some(prepared);
        self.last_ticks = 0;
        self.external_subtitle_track_ids.clear();
        if let Some(mpv) = self.mpv.as_mut()
            && let Err(e) = mpv.set_property("force-media-title", json!(title)).await
        {
            tracing::warn!("could not set mpv's media title: {e:#}");
        }
        self.send_start();
        Ok(())
    }

    pub(in crate::runtime) async fn stop_playback(&mut self, report: bool) {
        tracing::info!(
            item = %self.current.as_ref().map_or("?", |current| current.item_id.as_str()),
            position_s = jellysink_core::ticks::ticks_to_seconds(self.last_ticks),
            "stopping playback"
        );
        self.transitioning = false;
        // A stale resume offset must never seek a later item.
        self.pending_start_ticks = None;
        self.prepared.clear();
        self.titles.clear();
        self.window.clear();
        self.sample_position().await;
        if report {
            self.send_stopped();
        }
        if let Some(mpv) = self.mpv.take() {
            mpv.quit().await;
        }
        self.current = None;
        self.external_subtitle_track_ids.clear();
        self.audio.settled = SelectedTrack::Unresolved;
        self.subtitle.settled = SelectedTrack::Unresolved;
        self.paused = false;
        self.publish_status();
    }

    /// A failed or zero sample keeps the last known position: mpv reads 0
    /// while a file unloads, and fails once its window is closed.
    pub(in crate::runtime) async fn sample_position(&mut self) {
        let live = match self.mpv.as_mut() {
            Some(mpv) => mpv.time_pos().await.ok(),
            None => None,
        };
        self.last_ticks = jellysink_core::ticks::coalesce_position_ticks(live, self.last_ticks);
    }

    pub(in crate::runtime) async fn toggle_pause(&mut self) -> color_eyre::Result<()> {
        if let Some(mpv) = self.mpv.as_mut() {
            mpv.toggle_pause().await?;
            self.paused = mpv.paused().await.unwrap_or(self.paused);
            tracing::info!(paused = self.paused, "toggle pause");
            self.send_progress();
        }
        Ok(())
    }

    pub(in crate::runtime) async fn apply_pause(&mut self, paused: bool) -> color_eyre::Result<()> {
        if let Some(mpv) = self.mpv.as_mut() {
            mpv.set_pause(paused).await?;
            self.paused = paused;
            tracing::info!(paused, "pause");
            self.send_progress();
        }
        Ok(())
    }

    pub(in crate::runtime) async fn apply_volume(&mut self, volume: i64) -> color_eyre::Result<()> {
        self.volume = volume.clamp(0, 100);
        if let Some(mpv) = self.mpv.as_mut() {
            mpv.set_volume(self.volume).await?;
        }
        self.send_progress();
        Ok(())
    }

    pub(in crate::runtime) async fn bump_volume(&mut self, delta: i64) -> color_eyre::Result<()> {
        if let Some(mpv) = self.mpv.as_mut() {
            self.volume = mpv.add_volume(delta).await?;
        } else {
            self.volume = (self.volume + delta).clamp(0, 100);
        }
        self.send_progress();
        Ok(())
    }

    pub(in crate::runtime) async fn apply_mute(&mut self, muted: bool) -> color_eyre::Result<()> {
        self.muted = muted;
        tracing::info!(muted, "mute");
        if let Some(mpv) = self.mpv.as_mut() {
            mpv.set_mute(self.muted).await?;
        }
        self.send_progress();
        Ok(())
    }
}
