//! Series autoplay: what plays next and when the queue grows. See
//! `specs/playlist.md`.

pub(super) mod expand;
mod stubs;

use crate::media::{self, PlayRequest, PreparedPlay};
use crate::runtime::state::Runtime;
use crate::runtime::window::{PlaylistEof, playlist_eof};
use jellysink_core::jellyfin::auth::Api;
use serde_json::Value;

async fn fetch_prepared(
    api: &Api,
    item_id: &str,
    req: &PlayRequest,
) -> color_eyre::Result<(PreparedPlay, Option<Value>)> {
    let info_fut = crate::jellyfin::playback_info(api, item_id, req);
    let item_fut = api.get_item(item_id);
    let (info, item) = tokio::join!(info_fut, item_fut);
    let info = info?;
    let item = match item {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::debug!("could not fetch item metadata: {e:#}");
            None
        }
    };
    let mut prep = media::prepare_play(&api.server, item_id, &info, req, &api.token)?;
    if let Some(ref v) = item {
        prep.title = media::display_title(v);
    }
    Ok((prep, item))
}

impl Runtime {
    pub(in crate::runtime) fn log_queue(&self, at: &str) {
        tracing::info!(
            at,
            index = self.window.index(),
            queue = self.window.len(),
            current = self.window.current(),
            next = self.window.peek_next(),
            autoplay = self.config.autoplay,
            "queue"
        );
    }

    pub(in crate::runtime) async fn play_next_or_stop(&mut self, from_eof: bool) {
        self.log_queue("play-next-or-stop");
        let (playlist_pos, playlist_count) = match self.playlist_state().await {
            Ok(Some(state)) => state,
            Ok(None) => (0, 0),
            Err(e) => {
                // An unreadable playlist state means mpv is gone or wedged;
                // guessing would put the wrong episode on screen.
                tracing::error!("cannot read mpv playlist state: {e:#}; stopping");
                self.stop_playback(true).await;
                return;
            }
        };
        let expected_pos = self.window.expected_pos();
        tracing::info!(
            playlist_pos,
            playlist_count,
            expected_pos,
            head = self.window.head(),
            from_eof,
            has_next = self.window.has_next(),
            "eof playlist state"
        );
        match playlist_eof(
            playlist_pos,
            playlist_count,
            self.window.has_next(),
            expected_pos,
            from_eof,
        ) {
            PlaylistEof::NextInMpv => {
                tracing::info!(playlist_pos, playlist_count, expected_pos, "playlist-next");
                self.advance_in_mpv().await;
            }
            PlaylistEof::WaitForMpv => {
                tracing::info!(
                    playlist_pos,
                    playlist_count,
                    expected_pos,
                    "eof; mpv will play next (waiting for file-loaded)"
                );
            }
            PlaylistEof::NextNotInMpv => self.advance_in_queue().await,
            // Neither has anything left — which is exactly when the series may.
            PlaylistEof::Stop => self.expand_then_advance_or_stop().await,
        }
    }

    async fn advance_in_mpv(&mut self) {
        self.transitioning = true;
        let advanced = match self.mpv.as_mut() {
            Some(mpv) => {
                let advanced = mpv.playlist_next().await;
                if advanced.is_ok() {
                    let _ = mpv.unpause().await;
                }
                advanced
            }
            None => Ok(()),
        };
        if let Err(e) = advanced {
            // Nothing will emit file-loaded now, and a stuck flag makes
            // end_file_action ignore every later end-file.
            self.transitioning = false;
            tracing::error!("playlist-next failed: {e:#}");
        }
    }

    async fn advance_in_queue(&mut self) {
        if self.window.advance().is_some() {
            tracing::info!(item = self.window.current(), "advancing to queued next");
            self.start_next_item().await;
            return;
        }
        self.expand_then_advance_or_stop().await;
    }

    async fn expand_then_advance_or_stop(&mut self) {
        tracing::info!("queue exhausted; trying series expand");
        self.try_expand_from_playing_item().await;
        if self.window.advance().is_some() {
            tracing::info!(
                item = self.window.current(),
                "advancing after series expand"
            );
            self.start_next_item().await;
        } else {
            tracing::info!("no next episode; stopping");
            self.stop_playback(true).await;
        }
    }

    async fn start_next_item(&mut self) {
        if let Err(e) = self.start_current(&PlayRequest::default()).await {
            tracing::error!("next item failed: {e:#}");
        }
    }

    /// `None` when no mpv is running. `playlist_eof` decides autoplay from
    /// these, so a failed read is an error rather than a fabricated `(0, 0)`.
    pub(in crate::runtime) async fn playlist_state(
        &mut self,
    ) -> color_eyre::Result<Option<(usize, usize)>> {
        let Some(mpv) = self.mpv.as_mut() else {
            return Ok(None);
        };
        // mpv reports -1 for both while idle; clamp rather than treat as a failure.
        let pos = mpv.playlist_pos().await?.max(0) as usize;
        let count = mpv.playlist_count().await?.max(0) as usize;
        Ok(Some((pos, count)))
    }

    /// The one place a [`PreparedPlay`] is produced, so also the one place the
    /// remembered tracks are applied.
    pub(in crate::runtime) async fn prepare_item(
        &mut self,
        item_id: &str,
        req: &PlayRequest,
    ) -> color_eyre::Result<(PreparedPlay, Option<Value>)> {
        if req.is_plain()
            && let Some(prep) = self.prepared.get(item_id).cloned()
        {
            return Ok((self.with_remembered_tracks(prep, req), None));
        }

        let (prep, item) = fetch_prepared(&self.api, item_id, req).await?;
        if let Some(ref v) = item {
            self.titles
                .insert(item_id.to_string(), media::display_title(v));
        }
        // The server's answer, not the overridden one: a later fallback must
        // mean "what the server said", not an earlier play's preference.
        self.prepared.insert(item_id.to_string(), prep.clone());
        Ok((self.with_remembered_tracks(prep, req), item))
    }

    fn with_remembered_tracks(&self, mut prep: PreparedPlay, req: &PlayRequest) -> PreparedPlay {
        prep.subtitle_stream_index = media::resolve_subtitle_index(
            req.subtitle_stream_index,
            self.subtitle.remembered.as_ref(),
            &prep.maps.subtitles,
            prep.subtitle_stream_index,
        );
        prep.audio_stream_index = media::resolve_audio_index(
            req.audio_stream_index,
            self.audio.remembered.as_ref(),
            &prep.maps.audios,
            prep.audio_stream_index,
        );
        prep
    }
}
