//! Applying, adopting and remembering a track, parameterised over
//! `TrackKind` rather than written once per kind. See `specs/tracks.md`.

use crate::media::{TrackKind, TrackPreference};
use crate::mpv::SelectedTrack;
use crate::runtime::state::{Runtime, TrackState};
use jellysink_core::jellyfin::url::redact_api_key;

impl Runtime {
    pub(in crate::runtime) async fn configure_streams(&mut self) -> color_eyre::Result<()> {
        let Some(prepared) = self.current.as_ref() else {
            return Ok(());
        };
        let subtitle_urls = prepared.maps.subtitle_url.clone();
        let requested = [
            (TrackKind::Audio, prepared.audio_stream_index),
            (TrackKind::Subtitle, prepared.subtitle_stream_index),
        ];
        let Some(mpv) = self.mpv.as_mut() else {
            return Ok(());
        };

        self.external_subtitle_track_ids.clear();
        for (jellyfin_index, url) in subtitle_urls {
            let result = mpv.sub_add(&url).await;
            let url = redact_api_key(&url);
            if let Err(e) = result {
                tracing::warn!("sub-add failed for {url}: {e:#}");
                continue;
            }
            match mpv.max_subtitle_track_id().await {
                Ok(subtitle_track_id) => {
                    tracing::info!(
                        jellyfin_index,
                        mpv_subtitle_track_id = subtitle_track_id,
                        url = %url,
                        "loaded external subtitle track"
                    );
                    self.external_subtitle_track_ids
                        .insert(jellyfin_index, subtitle_track_id);
                }
                Err(e) => {
                    tracing::warn!(
                        "failed getting max_subtitle_track_id after sub-add for {url}: {e:#}"
                    );
                }
            }
        }

        for (kind, jellyfin_index) in requested {
            if let Some(jellyfin_index) = jellyfin_index {
                tracing::info!(
                    kind = kind.as_str(),
                    jellyfin_index,
                    "configuring initial stream"
                );
                if let Err(e) = self.apply_track(kind, jellyfin_index).await {
                    tracing::warn!(
                        kind = kind.as_str(),
                        jellyfin_index,
                        "could not configure initial stream: {e:#}"
                    );
                }
            }
        }
        // Whatever mpv ended up on is this file's baseline, so the property
        // changes the writes above emitted do not read as a user's pick.
        self.settle_track(TrackKind::Subtitle).await;
        self.settle_track(TrackKind::Audio).await;
        Ok(())
    }

    pub(in crate::runtime) fn track_state(&self, kind: TrackKind) -> &TrackState {
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
        match self.mpv.as_mut()?.selected_track(kind).await {
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

    /// Adopts a track picked in the mpv window, mapping mpv's track id back to
    /// a Jellyfin stream index against the last settled selection.
    pub(in crate::runtime) async fn adopt_mpv_track(&mut self, kind: TrackKind) {
        // A loading file reports neither the old selection nor the new one.
        if self.transitioning || self.current.is_none() {
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
                    // A track Jellyfin does not have (a user's own sidecar).
                    // Unreportable, but still the baseline so it stops re-firing.
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
        if let Some(prepared) = self.current.as_mut() {
            let stream_index = prepared.stream_index_mut(kind);
            tracing::info!(
                kind = kind.as_str(),
                jellyfin_index,
                previous = *stream_index,
                "track changed in mpv"
            );
            *stream_index = (jellyfin_index >= 0).then_some(jellyfin_index);
        }
        self.track_state_mut(kind).settled = selected;
        self.remember_track(kind, jellyfin_index);
        self.send_progress();
    }

    /// Records the user's choice by identity, since the next episode numbers
    /// its streams differently. An unidentifiable choice is forgotten.
    pub(in crate::runtime) fn remember_track(&mut self, kind: TrackKind, jellyfin_index: i64) {
        let candidates = self
            .current
            .as_ref()
            .map_or(&[][..], |prepared| prepared.maps.candidates(kind));
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
        let track_id = if jellyfin_index < 0 {
            tracing::info!(
                kind = kind.as_str(),
                jellyfin_index,
                "disabling track in mpv"
            );
            None
        } else {
            let Some(track_id) = self.mpv_track_id_for(kind, jellyfin_index) else {
                tracing::warn!(
                    kind = kind.as_str(),
                    jellyfin_index,
                    embedded = ?self.current.as_ref().map(|prepared| prepared.maps.track_ids(kind)),
                    external_subtitles = ?self.external_subtitle_track_ids,
                    "requested stream index is in neither the embedded nor the external track map"
                );
                self.set_current_stream_index(kind, Some(jellyfin_index));
                return Ok(());
            };
            tracing::info!(
                kind = kind.as_str(),
                jellyfin_index,
                mpv_track_id = track_id,
                "applied stream"
            );
            Some(track_id)
        };
        if let Some(mpv) = self.mpv.as_mut() {
            mpv.set_track_id(kind, track_id).await?;
        }
        // Ours, so the property change it triggers is not a user pick.
        self.track_state_mut(kind).settled = track_id.map_or(SelectedTrack::Off, SelectedTrack::Id);
        self.set_current_stream_index(kind, (jellyfin_index >= 0).then_some(jellyfin_index));
        Ok(())
    }

    fn set_current_stream_index(&mut self, kind: TrackKind, jellyfin_index: Option<i64>) {
        if let Some(prepared) = self.current.as_mut() {
            *prepared.stream_index_mut(kind) = jellyfin_index;
        }
    }

    /// The Jellyfin stream index an mpv track id came from. `sub-add` appends,
    /// so external subtitle ids sit above the embedded numbering.
    fn jellyfin_index_of(&self, kind: TrackKind, track_id: i64) -> Option<i64> {
        let external = match kind {
            TrackKind::Subtitle => Some(&self.external_subtitle_track_ids),
            TrackKind::Audio => None,
        };
        let embedded = self
            .current
            .as_ref()
            .map(|prepared| prepared.maps.track_ids(kind));
        external
            .into_iter()
            .chain(embedded)
            .flatten()
            .find(|(_, id)| **id == track_id)
            .map(|(jellyfin_index, _)| *jellyfin_index)
    }

    /// The mpv track id for a Jellyfin stream index — [`Self::jellyfin_index_of`]
    /// the other way round, external subtitles first for the same reason.
    fn mpv_track_id_for(&self, kind: TrackKind, jellyfin_index: i64) -> Option<i64> {
        if kind == TrackKind::Subtitle
            && let Some(track_id) = self.external_subtitle_track_ids.get(&jellyfin_index)
        {
            return Some(*track_id);
        }
        self.current
            .as_ref()?
            .maps
            .track_ids(kind)
            .get(&jellyfin_index)
            .copied()
    }
}
