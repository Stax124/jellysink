//! The Jellyfin-index <-> mpv-track-id maps and the remembered hand-picked
//! track. See `specs/tracks.md`.

use crate::media::{
    TrackId, TrackKind, TrackPreference, jellyfin_embedded_audio_index,
    jellyfin_embedded_subtitle_index, mpv_audio_track_id, mpv_embedded_subtitle_track_id,
};
use crate::mpv::SelectedTrack;
use crate::runtime::state::{Runtime, TrackState};
use std::collections::HashMap;

impl Runtime {
    pub(in crate::runtime) async fn configure_streams(&mut self) -> color_eyre::Result<()> {
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
    pub(in crate::runtime) async fn settle_track(&mut self, kind: TrackKind) {
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
    pub(in crate::runtime) async fn adopt_mpv_track(&mut self, kind: TrackKind) {
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
    pub(in crate::runtime) fn remember_track(&mut self, kind: TrackKind, jellyfin_index: i64) {
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
    pub(in crate::runtime) async fn apply_track(
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
}
