use super::Runtime;
use crate::jellyfin::auth::Api;
use crate::media::{
    PlayRequest, PreparedPlay, SubtitlePreference, jellyfin_embedded_subtitle_index,
    mpv_audio_track_id, mpv_embedded_subtitle_track_id, remember_subtitle_preference,
};
use crate::mpv::{MpvSession, SelectedTrack};
use crate::report::{PlayingState, Report};
use color_eyre::eyre::eyre;
use serde_json::json;

impl Runtime {
    pub(super) async fn start_current(&mut self, req: &PlayRequest) -> color_eyre::Result<()> {
        let Some(item_id) = self.window.current().map(str::to_string) else {
            self.stop_playback(true).await;
            return Ok(());
        };

        self.prepared.clear();
        self.titles.clear();
        self.window.reset_to_current();

        let reuse = self.mpv.is_some();
        if reuse {
            if let Some(mpv) = self.mpv.as_mut() {
                let live = mpv.time_pos().await.ok();
                self.last_ticks = crate::ticks::coalesce_position_ticks(live, self.last_ticks);
            }
            self.send_stopped();
            self.transitioning = true;
        }

        let (prep, item) = match self.prepare_item(&item_id, req).await {
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

        if reuse {
            if let Err(e) = self.load_into_existing(&prep, &item_id).await {
                tracing::error!("{e:#}");
                self.stop_playback(false).await;
                return Ok(());
            }
        } else if let Err(e) = self.spawn_and_load(&prep, &item_id).await {
            tracing::error!("{e:#}");
            return Ok(());
        }

        self.current = Some(prep);
        self.item_id = Some(item_id);
        self.paused = false;
        self.stopping = false;
        self.last_ticks = req.start_ticks.unwrap_or(0);
        self.external_subtitle_track_ids.clear();

        self.pending_start_ticks = resume_seek_ticks(req.start_ticks);

        self.send_start();
        tracing::info!(
            item = %self.item_id.as_deref().unwrap_or("?"),
            title = %self.current.as_ref().map(|p| p.title.as_str()).unwrap_or("?"),
            queue = self.window.len(),
            index = self.window.index(),
            "playing"
        );
        // After `loadfile ... replace` (which wipes mpv's playlist), put the
        // rest of the window in: remaining episodes at the end, previous
        // episodes spliced in ahead of the current file.
        self.fill_forward_into_mpv().await;
        self.fill_previous_into_mpv().await;
        Ok(())
    }

    pub(super) async fn adopt_playlist_pos(&mut self) -> color_eyre::Result<()> {
        let playlist_pos = self
            .mpv
            .as_mut()
            .ok_or_else(|| eyre!("mpv missing"))?
            .playlist_pos()
            .await?
            .max(0) as usize;
        let Some(queue_index) = self.window.queue_index_at(playlist_pos) else {
            return Ok(());
        };
        let item_id = self.window.items()[queue_index].clone();
        if self.item_id.as_deref() == Some(item_id.as_str()) {
            return Ok(());
        }
        self.send_stopped();
        self.window.adopt_index(queue_index);
        tracing::info!(item = %item_id, index = queue_index, "adopted mpv playlist jump");
        // No cache branch here. `prepare_item` already returns the cached
        // prepare for a plain request, and routing through it is what lets the
        // remembered subtitle reach a playlist jump and mpv's own autoplay —
        // which is the path a series actually takes between episodes.
        let (prep, _) = self.prepare_item(&item_id, &PlayRequest::default()).await?;
        self.current = Some(prep);
        self.item_id = Some(item_id);
        self.last_ticks = 0;
        self.external_subtitle_track_ids.clear();
        if let Some(title) = self.current.as_ref().map(|p| p.title.clone())
            && let Some(mpv) = self.mpv.as_mut()
        {
            let _ = mpv.set_property("force-media-title", json!(title)).await;
        }
        self.send_start();
        Ok(())
    }

    pub(super) async fn configure_streams(&mut self) -> color_eyre::Result<()> {
        let Some(prep) = self.current.clone() else {
            return Ok(());
        };
        let Some(mpv) = self.mpv.as_mut() else {
            return Ok(());
        };

        self.external_subtitle_track_ids.clear();
        for (jellyfin_index, url) in &prep.external_sub_urls {
            if let Err(e) = mpv.sub_add(url).await {
                tracing::warn!("sub-add failed for {url}: {e:#}");
                continue;
            }
            match mpv.max_subtitle_track_id().await {
                Ok(subtitle_track_id) => {
                    tracing::info!(
                        jellyfin_index = *jellyfin_index,
                        mpv_subtitle_track_id = subtitle_track_id,
                        url = %url,
                        "loaded external subtitle track"
                    );
                    self.external_subtitle_track_ids
                        .insert(*jellyfin_index, subtitle_track_id);
                }
                Err(e) => {
                    tracing::warn!(
                        "failed getting max_subtitle_track_id after sub-add for {url}: {e:#}"
                    );
                }
            }
        }

        if let Some(audio_track_id) = prep
            .audio_stream_index
            .and_then(|i| mpv_audio_track_id(&prep.maps, i))
        {
            let _ = mpv.set_audio_track_id(audio_track_id).await;
        }
        if let Some(subtitle_stream_index) = prep.subtitle_stream_index {
            tracing::info!(
                jellyfin_subtitle_index = subtitle_stream_index,
                "configuring initial subtitle stream"
            );
            let _ = self.apply_subtitle(subtitle_stream_index).await;
        }
        // Whatever mpv ended up on is the baseline for this file, so the
        // property changes `sub-add` and our own `sid` writes just emitted are
        // not mistaken for the user reaching for the track menu. Read rather
        // than assumed: with no index to apply, the selection is mpv's own —
        // its config's default track, or the last `sub-add`ed one.
        self.settle_subtitle_track().await;
        Ok(())
    }

    /// Records mpv's current subtitle selection as *not* a user choice.
    pub(super) async fn settle_subtitle_track(&mut self) {
        let Some(mpv) = self.mpv.as_mut() else {
            return;
        };
        match mpv.subtitle_track().await {
            Ok(track) => self.settled_subtitle_track = track,
            Err(e) => tracing::debug!("could not read mpv subtitle track: {e:#}"),
        }
    }

    /// Adopts a subtitle track picked in the mpv window.
    ///
    /// A Jellyfin client is not the only way to change subtitles — `j` in mpv
    /// is, and in practice it is the usual one. mpv reports it as a `sid`
    /// property change; this maps that track id back to the Jellyfin stream
    /// index the rest of the code speaks, so the choice is remembered for the
    /// next episode ([`Runtime::remember_subtitle`]) and the Jellyfin UI stops
    /// showing a track that is not playing.
    ///
    /// The event's own value is not used. Property changes travel on their own
    /// channel and are handled well after they were emitted, so mpv's
    /// auto-selection during a file load lands here *after*
    /// [`Runtime::configure_streams`] has applied our choice over it. Comparing
    /// mpv's live selection against the one we last settled on is what makes
    /// those stale events no-ops.
    pub(super) async fn adopt_mpv_subtitle_track(&mut self) {
        // A file that is still loading reports the selection of neither the old
        // file nor the configured new one.
        if self.transitioning || self.stopping || self.current.is_none() {
            return;
        }
        let Some(mpv) = self.mpv.as_mut() else {
            return;
        };
        let selected = match mpv.subtitle_track().await {
            Ok(track) => track,
            Err(e) => {
                tracing::debug!("could not read mpv subtitle track: {e:#}");
                return;
            }
        };
        if selected == self.settled_subtitle_track {
            return;
        }
        let jellyfin_index = match selected {
            // mpv between tracks, not a decision to report.
            SelectedTrack::Unresolved => return,
            SelectedTrack::Off => -1,
            SelectedTrack::Id(subtitle_track_id) => {
                match self.jellyfin_subtitle_index(subtitle_track_id) {
                    Some(jellyfin_index) => jellyfin_index,
                    None => {
                        // A track mpv has and Jellyfin does not: a sidecar the
                        // user loaded themselves, or an in-file track Jellyfin
                        // only offers as an extracted sidecar. Nothing to
                        // report, and nothing that could be re-found in the
                        // next episode — but it is what is on screen, so it
                        // becomes the baseline and stops re-firing.
                        tracing::debug!(
                            subtitle_track_id,
                            "mpv selected a subtitle track with no Jellyfin stream index"
                        );
                        self.settled_subtitle_track = selected;
                        return;
                    }
                }
            }
        };
        tracing::info!(
            jellyfin_index,
            previous = self.current.as_ref().and_then(|p| p.subtitle_stream_index),
            "subtitle track changed in mpv"
        );
        self.settled_subtitle_track = selected;
        self.remember_subtitle(jellyfin_index);
        if let Some(prep) = self.current.as_mut() {
            prep.subtitle_stream_index = (jellyfin_index >= 0).then_some(jellyfin_index);
        }
        self.send_progress();
    }

    /// The Jellyfin stream index an mpv subtitle track id came from.
    fn jellyfin_subtitle_index(&self, subtitle_track_id: i64) -> Option<i64> {
        // `sub-add` appends, so an external track's id sits above the embedded
        // numbering and the two cannot collide; the order here is arbitrary.
        self.external_subtitle_track_ids
            .iter()
            .find(|(_, track_id)| **track_id == subtitle_track_id)
            .map(|(jellyfin_index, _)| *jellyfin_index)
            .or_else(|| {
                jellyfin_embedded_subtitle_index(&self.current.as_ref()?.maps, subtitle_track_id)
            })
    }

    /// Records the user's subtitle choice by identity, so the next episode can
    /// get the same track even though its stream index will differ.
    ///
    /// A choice we cannot identify is *forgotten* rather than kept: an identity
    /// that can never match again would leave the previous choice in place, and
    /// silently re-applying a track the user has already moved away from is
    /// worse than falling back to the server default.
    pub(super) fn remember_subtitle(&self, jellyfin_index: i64) {
        let candidates = self
            .current
            .as_ref()
            .map_or(&[][..], |prep| prep.maps.subtitles.as_slice());
        let preference = SubtitlePreference::from_selection(candidates, jellyfin_index);
        match &preference {
            Some(SubtitlePreference::Off) => {
                tracing::info!("remembering subtitles off for the next episode");
            }
            Some(SubtitlePreference::Stream(id)) => tracing::info!(
                jellyfin_index,
                language = id.language.as_deref(),
                title = id.title.as_deref(),
                display_title = id.display_title.as_deref(),
                "remembering subtitle track for the next episode"
            ),
            None => tracing::debug!(
                jellyfin_index,
                candidates = candidates.len(),
                "subtitle choice carries no identity; forgetting the previous one"
            ),
        }
        remember_subtitle_preference(&self.last_subtitle, preference);
    }

    pub(super) async fn apply_subtitle(&mut self, jellyfin_index: i64) -> color_eyre::Result<()> {
        let Some(mpv) = self.mpv.as_mut() else {
            return Ok(());
        };
        if jellyfin_index < 0 {
            tracing::info!(jellyfin_index, "disabling subtitles in mpv (sid=no)");
            mpv.set_subtitle_track_id(None).await?;
            // Ours, so the property change it triggers is not a user pick.
            self.settled_subtitle_track = SelectedTrack::Off;
            if let Some(prep) = self.current.as_mut() {
                prep.subtitle_stream_index = None;
            }
            return Ok(());
        }
        if let Some(subtitle_track_id) = self
            .external_subtitle_track_ids
            .get(&jellyfin_index)
            .copied()
        {
            tracing::info!(
                jellyfin_index,
                mpv_subtitle_track_id = subtitle_track_id,
                "applied external subtitle stream"
            );
            mpv.set_subtitle_track_id(Some(subtitle_track_id)).await?;
            self.settled_subtitle_track = SelectedTrack::Id(subtitle_track_id);
        } else if let Some(prep) = self.current.as_ref()
            && let Some(subtitle_track_id) =
                mpv_embedded_subtitle_track_id(&prep.maps, jellyfin_index)
        {
            tracing::info!(
                jellyfin_index,
                mpv_subtitle_track_id = subtitle_track_id,
                "applied embedded subtitle stream"
            );
            mpv.set_subtitle_track_id(Some(subtitle_track_id)).await?;
            self.settled_subtitle_track = SelectedTrack::Id(subtitle_track_id);
        } else {
            tracing::warn!(
                jellyfin_index,
                external_map = ?self.external_subtitle_track_ids,
                embedded_map = ?self.current.as_ref().map(|p| &p.maps.subtitle_track_id_by_stream_index),
                "requested subtitle stream index not found in external or embedded subtitle maps"
            );
        }
        if let Some(prep) = self.current.as_mut() {
            prep.subtitle_stream_index = Some(jellyfin_index);
        }
        Ok(())
    }

    pub(super) async fn tick_progress(&mut self) {
        if self.mpv.is_none() || self.current.is_none() {
            return;
        }
        if let Some(mpv) = self.mpv.as_mut() {
            // A dead/zero sample during unload must not throw away a known position.
            let live = mpv.time_pos().await.ok();
            self.last_ticks = crate::ticks::coalesce_position_ticks(live, self.last_ticks);
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
        self.send_progress();
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

    fn send_start(&self) {
        if let Some(s) = self.snapshot(self.last_ticks) {
            let _ = self.report_tx.send(Report::Start(s));
        }
    }

    pub(super) fn send_progress(&self) {
        if let Some(s) = self.snapshot(self.last_ticks) {
            let _ = self.report_tx.send(Report::Progress(s));
        }
    }

    fn send_stopped(&self) {
        if let Some(mut s) = self.snapshot(self.last_ticks) {
            s.is_paused = true;
            let _ = self.report_tx.send(Report::Stopped(s));
        }
    }

    async fn load_into_existing(
        &mut self,
        prep: &PreparedPlay,
        item_id: &str,
    ) -> color_eyre::Result<()> {
        let Some(mpv) = self.mpv.as_mut() else {
            return Err(eyre!("mpv missing during reuse"));
        };
        let auth = apply_auth(&self.api, mpv, prep, item_id, self.window.has_next()).await;
        mpv.loadfile(&auth.url, Some(prep.title.as_str())).await?;
        self.mpv_auth_header_set = auth.header_set;
        let _ = mpv.set_volume(self.volume).await;
        let _ = mpv.set_mute(self.muted).await;
        let _ = mpv.unpause().await;
        Ok(())
    }

    async fn spawn_and_load(
        &mut self,
        prep: &PreparedPlay,
        item_id: &str,
    ) -> color_eyre::Result<()> {
        // Re-read mpv_args on every spawn so edits apply to the next play
        // without restarting the daemon.
        let mpv_args = crate::config::MpvArgs::load(&self.paths)
            .inspect_err(|e| {
                tracing::warn!("mpv_args unreadable; spawning without extra args: {e:#}");
            })
            .unwrap_or_default();
        let (mut mpv, events) =
            MpvSession::spawn(&self.config.mpv_path, &mpv_args.0, self.paths.mpv_socket()).await?;
        mpv.set_keep_open().await?;
        if let Err(e) = mpv.observe_subtitle_track().await {
            tracing::warn!(
                "cannot observe mpv's subtitle track ({e:#}); a track picked in the mpv \
                 window will not be remembered or reported"
            );
        }
        tracing::info!("mpv spawned");

        let auth = apply_auth(&self.api, &mut mpv, prep, item_id, self.window.has_next()).await;
        if let Err(e) = mpv.loadfile(&auth.url, Some(prep.title.as_str())).await {
            let _ = mpv.quit_and_wait().await;
            return Err(e);
        }
        self.mpv_auth_header_set = auth.header_set;
        let _ = mpv.set_volume(self.volume).await;
        let _ = mpv.set_mute(self.muted).await;

        self.mpv_gen = self.mpv_gen.wrapping_add(1);
        let generation = self.mpv_gen;
        let tx = self.mpv_tx.clone();
        // Aborting the previous forwarder stops it leaking, but is not enough on
        // its own: events it already put on the shared channel are still queued.
        // `generation` is what lets the main loop discard those.
        if let Some(previous) = self.mpv_events.replace(tokio::spawn(async move {
            let mut events = events;
            while let Some(ev) = events.recv().await {
                if tx.send((generation, ev)).is_err() {
                    break;
                }
            }
        })) {
            previous.abort();
        }
        self.mpv = Some(mpv);
        self.transitioning = true;
        Ok(())
    }

    pub(super) async fn stop_playback(&mut self, report: bool) {
        tracing::info!(
            item = %self.item_id.as_deref().unwrap_or("?"),
            position_s = crate::ticks::ticks_to_seconds(self.last_ticks),
            "stopping playback"
        );
        self.stopping = true;
        self.transitioning = false;
        // A stale resume offset must never seek a later item.
        self.pending_start_ticks = None;
        self.prepared.clear();
        self.titles.clear();
        self.window.clear();
        let live = if let Some(mpv) = self.mpv.as_mut() {
            mpv.time_pos().await.ok()
        } else {
            None
        };
        // A teardown sample can fail or read 0 (window closed, IPC gone);
        // never let it clobber the position the progress ticks already saved.
        self.last_ticks = crate::ticks::coalesce_position_ticks(live, self.last_ticks);
        if report {
            self.send_stopped();
        }
        if let Some(mut mpv) = self.mpv.take() {
            let _ = mpv.quit_and_wait().await;
        }
        self.mpv_gen = self.mpv_gen.wrapping_add(1);
        if let Some(events) = self.mpv_events.take() {
            events.abort();
        }
        self.current = None;
        self.item_id = None;
        self.external_subtitle_track_ids.clear();
        self.settled_subtitle_track = SelectedTrack::Unresolved;
        self.paused = false;
        self.stopping = false;
    }

    pub(super) async fn toggle_pause(&mut self) -> color_eyre::Result<()> {
        if let Some(mpv) = self.mpv.as_mut() {
            mpv.toggle_pause().await?;
            self.paused = mpv.paused().await.unwrap_or(self.paused);
            tracing::info!(paused = self.paused, "toggle pause");
            self.send_progress();
        }
        Ok(())
    }

    pub(super) async fn apply_pause(&mut self, paused: bool) -> color_eyre::Result<()> {
        if let Some(mpv) = self.mpv.as_mut() {
            if paused {
                mpv.pause().await?;
            } else {
                mpv.unpause().await?;
            }
            self.paused = paused;
            tracing::info!(paused, "pause");
            self.send_progress();
        }
        Ok(())
    }

    pub(super) async fn apply_volume(&mut self, volume: i64) -> color_eyre::Result<()> {
        self.volume = volume.clamp(0, 100);
        if let Some(mpv) = self.mpv.as_mut() {
            mpv.set_volume(self.volume).await?;
        }
        self.send_progress();
        Ok(())
    }

    pub(super) async fn bump_volume(&mut self, delta: i64) -> color_eyre::Result<()> {
        if let Some(mpv) = self.mpv.as_mut() {
            self.volume = mpv.add_volume(delta).await?;
        } else {
            self.volume = (self.volume + delta).clamp(0, 100);
        }
        self.send_progress();
        Ok(())
    }

    pub(super) async fn apply_mute(&mut self, muted: bool) -> color_eyre::Result<()> {
        self.muted = muted;
        tracing::info!(muted, "mute");
        if let Some(mpv) = self.mpv.as_mut() {
            mpv.set_mute(self.muted).await?;
        }
        self.send_progress();
        Ok(())
    }
}

fn stream_url_with_token(api: &Api, item_id: &str, prep: &PreparedPlay) -> String {
    if prep.url.contains("ApiKey=") {
        prep.url.clone()
    } else {
        crate::jellyfin::url::direct_stream_url(
            &api.server,
            item_id,
            &prep.media_source_id,
            prep.live_stream_id.as_deref(),
            Some(&api.token),
        )
    }
}

/// The URL to hand mpv, plus whether mpv is now carrying the Authorization
/// header.
///
/// The header is a global mpv property, so it also covers playlist rows loaded
/// later — which is how [`Runtime::playlist_stub_entries`] avoids putting the
/// token in URLs mpv writes to its watch_later files.
struct AppliedAuth {
    url: String,
    header_set: bool,
}

async fn apply_auth(
    api: &Api,
    mpv: &mut MpvSession,
    prep: &PreparedPlay,
    item_id: &str,
    force_url_token: bool,
) -> AppliedAuth {
    if !force_url_token && prep.uses_auth_header {
        match mpv.apply_auth_header(&api.mpv_auth_header_field()).await {
            Ok(()) => {
                return AppliedAuth {
                    url: prep.url.clone(),
                    header_set: true,
                };
            }
            Err(e) => {
                tracing::warn!("could not set mpv auth header ({e:#}); putting ApiKey on the URL");
                return AppliedAuth {
                    url: stream_url_with_token(api, item_id, prep),
                    header_set: false,
                };
            }
        }
    }
    let _ = mpv.clear_auth_header().await;
    AppliedAuth {
        url: stream_url_with_token(api, item_id, prep),
        header_set: false,
    }
}

/// Resume offsets only apply when positive; `None`/`0`/negative mean "start
/// from the beginning".
pub(super) fn resume_seek_ticks(start_ticks: Option<i64>) -> Option<i64> {
    start_ticks.filter(|t| *t > 0)
}

#[cfg(test)]
mod tests {
    use super::resume_seek_ticks;

    #[test]
    fn resume_seek_ticks_applies_only_positive_offsets() {
        assert_eq!(resume_seek_ticks(None), None);
        assert_eq!(resume_seek_ticks(Some(0)), None);
        assert_eq!(resume_seek_ticks(Some(-1)), None);
        assert_eq!(resume_seek_ticks(Some(1)), Some(1));
        // Halfway through a 20-minute episode.
        assert_eq!(resume_seek_ticks(Some(600_000_000)), Some(600_000_000));
    }
}
