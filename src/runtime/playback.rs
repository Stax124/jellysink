use super::state::{Runtime, TrackState};
use super::task::AbortOnDrop;
use crate::jellyfin::auth::Api;
use crate::media::{
    PlayRequest, PreparedPlay, TrackId, TrackKind, TrackPreference, jellyfin_embedded_audio_index,
    jellyfin_embedded_subtitle_index, mpv_audio_track_id, mpv_embedded_subtitle_track_id,
};
use crate::mpv::{MpvSession, SelectedTrack};
use crate::report::{PlayingState, Report};
use color_eyre::eyre::eyre;
use serde_json::json;
use std::collections::HashMap;

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
        // `loadfile ... replace` wiped mpv's playlist, so refill it around the
        // current file.
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
        // Via `prepare_item` (which caches plain requests itself) so remembered
        // tracks also reach a playlist jump and mpv's own autoplay.
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

        for (kind, jellyfin_index) in [
            (TrackKind::Audio, prep.audio_stream_index),
            (TrackKind::Subtitle, prep.subtitle_stream_index),
        ] {
            if let Some(jellyfin_index) = jellyfin_index {
                tracing::info!(
                    kind = kind.as_str(),
                    jellyfin_index,
                    "configuring initial stream"
                );
                let _ = self.apply_track(kind, jellyfin_index).await;
            }
        }
        // Whatever mpv ended up on is this file's baseline, so the property
        // changes the writes above emitted do not read as a user's pick.
        self.settle_track(TrackKind::Subtitle).await;
        self.settle_track(TrackKind::Audio).await;
        Ok(())
    }

    /// The two slots, addressed by kind. This — plus the small accessors below
    /// — is all that keeps the audio and subtitle paths from being two copies
    /// of the same code, as they were before.
    fn track_state(&self, kind: TrackKind) -> &TrackState {
        match kind {
            TrackKind::Audio => &self.audio,
            TrackKind::Subtitle => &self.subtitle,
        }
    }

    fn track_state_mut(&mut self, kind: TrackKind) -> &mut TrackState {
        match kind {
            TrackKind::Audio => &mut self.audio,
            TrackKind::Subtitle => &mut self.subtitle,
        }
    }

    /// mpv's live `aid`/`sid`, or `None` when there is no mpv or the read fails.
    async fn read_mpv_track(&mut self, kind: TrackKind) -> Option<SelectedTrack> {
        let mpv = self.mpv.as_mut()?;
        let read = match kind {
            TrackKind::Audio => mpv.audio_track().await,
            TrackKind::Subtitle => mpv.subtitle_track().await,
        };
        match read {
            Ok(track) => Some(track),
            Err(e) => {
                tracing::debug!(kind = kind.as_str(), "could not read mpv track: {e:#}");
                None
            }
        }
    }

    /// Records mpv's current selection as *not* a user choice.
    pub(super) async fn settle_track(&mut self, kind: TrackKind) {
        if let Some(track) = self.read_mpv_track(kind).await {
            self.track_state_mut(kind).settled = track;
        }
    }

    /// Adopts a track picked in the mpv window (`#` for audio, `j` for
    /// subtitles), mapping mpv's track id back to the Jellyfin stream index the
    /// rest of the code speaks.
    ///
    /// The event carries no value: it is handled long after it was emitted, so
    /// only mpv's live selection against the last settled one says anything.
    pub(super) async fn adopt_mpv_track(&mut self, kind: TrackKind) {
        // A loading file reports neither the old selection nor the new one.
        if self.transitioning || self.stopping || self.current.is_none() {
            return;
        }
        let Some(selected) = self.read_mpv_track(kind).await else {
            return;
        };
        if selected == self.track_state(kind).settled {
            return;
        }
        let jellyfin_index = match selected {
            // mpv between tracks, not a decision to report.
            SelectedTrack::Unresolved => return,
            // `cycle audio` past the last track: a decision like any other.
            SelectedTrack::Off => -1,
            SelectedTrack::Id(track_id) => match self.jellyfin_index_of(kind, track_id) {
                Some(jellyfin_index) => jellyfin_index,
                None => {
                    // A track Jellyfin does not have (a user's own sidecar, or
                    // an external file mpv picked up). Unreportable, but still
                    // the baseline so it stops re-firing.
                    tracing::debug!(
                        kind = kind.as_str(),
                        track_id,
                        "mpv selected a track with no Jellyfin stream index"
                    );
                    self.track_state_mut(kind).settled = selected;
                    return;
                }
            },
        };
        tracing::info!(
            kind = kind.as_str(),
            jellyfin_index,
            previous = self.prep_index(kind),
            "track changed in mpv"
        );
        self.track_state_mut(kind).settled = selected;
        self.remember_track(kind, jellyfin_index);
        self.set_prep_index(kind, (jellyfin_index >= 0).then_some(jellyfin_index));
        self.send_progress();
    }

    /// Records the user's choice by identity, since the next episode numbers
    /// its streams differently. An unidentifiable choice is forgotten rather
    /// than kept: re-applying a track the user has already moved away from is
    /// worse than falling back to the server default.
    pub(super) fn remember_track(&mut self, kind: TrackKind, jellyfin_index: i64) {
        let candidates = self.candidates(kind);
        let preference = TrackPreference::from_selection(candidates, jellyfin_index);
        match &preference {
            Some(TrackPreference::Off) => {
                tracing::info!(kind = kind.as_str(), "remembering off for the next episode")
            }
            Some(TrackPreference::Stream(id)) => tracing::info!(
                kind = kind.as_str(),
                jellyfin_index,
                language = id.language.as_deref(),
                title = id.title.as_deref(),
                display_title = id.display_title.as_deref(),
                "remembering track for the next episode"
            ),
            None => tracing::debug!(
                kind = kind.as_str(),
                jellyfin_index,
                candidates = candidates.len(),
                "choice carries no identity; forgetting the previous one"
            ),
        }
        self.track_state_mut(kind).remembered = preference;
    }

    /// Points mpv at a Jellyfin stream index. A negative index is an explicit
    /// off (`aid=no` / `sid=no`), not "unspecified".
    pub(super) async fn apply_track(
        &mut self,
        kind: TrackKind,
        jellyfin_index: i64,
    ) -> color_eyre::Result<()> {
        if self.mpv.is_none() {
            return Ok(());
        }
        if jellyfin_index < 0 {
            tracing::info!(
                kind = kind.as_str(),
                jellyfin_index,
                "disabling track in mpv"
            );
            self.set_mpv_track_id(kind, None).await?;
            // Ours, so the property change it triggers is not a user pick.
            self.track_state_mut(kind).settled = SelectedTrack::Off;
            self.set_prep_index(kind, None);
            return Ok(());
        }
        match self.mpv_track_id_for(kind, jellyfin_index) {
            Some(track_id) => {
                tracing::info!(
                    kind = kind.as_str(),
                    jellyfin_index,
                    mpv_track_id = track_id,
                    "applied stream"
                );
                self.set_mpv_track_id(kind, Some(track_id)).await?;
                self.track_state_mut(kind).settled = SelectedTrack::Id(track_id);
            }
            None => tracing::warn!(
                kind = kind.as_str(),
                jellyfin_index,
                embedded_map = ?self.embedded_map(kind),
                external_subtitles = ?self.external_subtitle_track_ids,
                "requested stream index is in neither the embedded nor the external track map"
            ),
        }
        self.set_prep_index(kind, Some(jellyfin_index));
        Ok(())
    }

    async fn set_mpv_track_id(
        &mut self,
        kind: TrackKind,
        track_id: Option<i64>,
    ) -> color_eyre::Result<()> {
        let Some(mpv) = self.mpv.as_mut() else {
            return Ok(());
        };
        match kind {
            TrackKind::Audio => mpv.set_audio_track_id(track_id).await,
            TrackKind::Subtitle => mpv.set_subtitle_track_id(track_id).await,
        }
    }

    /// The Jellyfin stream index an mpv track id came from.
    ///
    /// `sub-add` appends, so external subtitle ids sit above the embedded
    /// numbering and the two lookups cannot collide. Audio has one lookup
    /// rather than two: there is no external audio.
    fn jellyfin_index_of(&self, kind: TrackKind, track_id: i64) -> Option<i64> {
        if kind == TrackKind::Subtitle
            && let Some(jellyfin_index) = self
                .external_subtitle_track_ids
                .iter()
                .find(|(_, id)| **id == track_id)
                .map(|(jellyfin_index, _)| *jellyfin_index)
        {
            return Some(jellyfin_index);
        }
        let maps = &self.current.as_ref()?.maps;
        match kind {
            TrackKind::Audio => jellyfin_embedded_audio_index(maps, track_id),
            TrackKind::Subtitle => jellyfin_embedded_subtitle_index(maps, track_id),
        }
    }

    /// The mpv track id for a Jellyfin stream index — [`Self::jellyfin_index_of`]
    /// the other way round, external subtitles first for the same reason.
    fn mpv_track_id_for(&self, kind: TrackKind, jellyfin_index: i64) -> Option<i64> {
        if kind == TrackKind::Subtitle
            && let Some(track_id) = self
                .external_subtitle_track_ids
                .get(&jellyfin_index)
                .copied()
        {
            return Some(track_id);
        }
        let maps = &self.current.as_ref()?.maps;
        match kind {
            TrackKind::Audio => mpv_audio_track_id(maps, jellyfin_index),
            TrackKind::Subtitle => mpv_embedded_subtitle_track_id(maps, jellyfin_index),
        }
    }

    /// The embedded index → track id map, for the log line when a requested
    /// index is not in it.
    fn embedded_map(&self, kind: TrackKind) -> Option<&HashMap<i64, i64>> {
        let maps = &self.current.as_ref()?.maps;
        Some(match kind {
            TrackKind::Audio => &maps.audio_track_id_by_stream_index,
            TrackKind::Subtitle => &maps.subtitle_track_id_by_stream_index,
        })
    }

    /// Every stream of this kind the current item offers, for matching a
    /// remembered choice against.
    fn candidates(&self, kind: TrackKind) -> &[TrackId] {
        let Some(prep) = self.current.as_ref() else {
            return &[];
        };
        match kind {
            TrackKind::Audio => &prep.maps.audios,
            TrackKind::Subtitle => &prep.maps.subtitles,
        }
    }

    /// The Jellyfin stream index this kind is currently playing.
    fn prep_index(&self, kind: TrackKind) -> Option<i64> {
        let prep = self.current.as_ref()?;
        match kind {
            TrackKind::Audio => prep.audio_stream_index,
            TrackKind::Subtitle => prep.subtitle_stream_index,
        }
    }

    fn set_prep_index(&mut self, kind: TrackKind, jellyfin_index: Option<i64>) {
        let Some(prep) = self.current.as_mut() else {
            return;
        };
        match kind {
            TrackKind::Audio => prep.audio_stream_index = jellyfin_index,
            TrackKind::Subtitle => prep.subtitle_stream_index = jellyfin_index,
        }
    }

    pub(super) async fn tick_progress(&mut self) {
        if self.mpv.is_none() || self.current.is_none() {
            return;
        }
        self.sample_mpv_state().await;
        self.send_progress();
    }

    /// Re-announces the current play on a freshly connected session: a server
    /// that dropped the session needs a start to show a now-playing again.
    /// Resampled first, since nothing polled mpv while the socket was down.
    pub(super) async fn reannounce(&mut self) {
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
        let mpv_args = crate::app::config::MpvArgs::load(&self.paths)
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
        if let Err(e) = mpv.observe_audio_track().await {
            tracing::warn!(
                "cannot observe mpv's audio track ({e:#}); a track picked in the mpv \
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
        // The assignment drops — and so aborts — the previous forwarder. The
        // events it already queued stay on the shared channel; `generation` is
        // how the main loop discards those.
        self.mpv_events = Some(AbortOnDrop(tokio::spawn(async move {
            let mut events = events;
            while let Some(ev) = events.recv().await {
                if tx.send((generation, ev)).is_err() {
                    break;
                }
            }
        })));
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
        // A teardown sample can fail or read 0 (window closed, IPC gone).
        self.last_ticks = crate::ticks::coalesce_position_ticks(live, self.last_ticks);
        if report {
            self.send_stopped();
        }
        if let Some(mut mpv) = self.mpv.take() {
            let _ = mpv.quit_and_wait().await;
        }
        self.mpv_gen = self.mpv_gen.wrapping_add(1);
        // Dropped, and so aborted.
        self.mpv_events = None;
        self.current = None;
        self.item_id = None;
        self.external_subtitle_track_ids.clear();
        // Per-mpv-session, unlike the remembered tracks.
        self.audio.settled = SelectedTrack::Unresolved;
        self.subtitle.settled = SelectedTrack::Unresolved;
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

/// The URL to hand mpv, plus whether mpv now carries the Authorization header.
/// The header is global, so it covers later playlist rows too and keeps the
/// token out of the URLs mpv writes to its watch_later files.
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
#[path = "playback_test.rs"]
mod tests;
