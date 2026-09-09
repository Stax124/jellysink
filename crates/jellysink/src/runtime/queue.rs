use super::state::Runtime;
use crate::media::{self, PlayRequest, PreparedPlay};
use crate::runtime::window::{PlaylistEof, playlist_eof};
use jellysink_core::jellyfin::auth::Api;
use serde_json::Value;
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fill {
    Append,
    /// At position 0. Does not interrupt playback; mpv shifts `playlist-pos`.
    Prepend,
    /// Right after the current item, at the mpv position
    /// `PlaylistWindow::insert_next` returned. Same non-interrupting splice as
    /// `Prepend`, just not pinned to 0.
    Next(usize),
}

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
    pub(super) fn log_queue(&self, at: &str) {
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

    pub(super) async fn play_next_or_stop(&mut self, from_eof: bool) {
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
    pub(super) async fn playlist_state(&mut self) -> color_eyre::Result<Option<(usize, usize)>> {
        let Some(mpv) = self.mpv.as_mut() else {
            return Ok(None);
        };
        // mpv reports -1 for both while idle; clamp rather than treat as a failure.
        let pos = mpv.playlist_pos().await?.max(0) as usize;
        let count = mpv.playlist_count().await?.max(0) as usize;
        Ok(Some((pos, count)))
    }

    async fn try_expand_from_playing_item(&mut self) {
        let Some(item_id) = self.item_id.clone() else {
            return;
        };
        let item = match self.api.get_item(&item_id).await {
            Ok(v) => v,
            Err(e) => {
                tracing::info!("could not re-fetch item for series expand: {e:#}");
                return;
            }
        };
        self.maybe_expand_series(&item, &item_id).await;
    }

    pub(super) async fn maybe_expand_series(&mut self, item: &Value, current_id: &str) {
        let item_type = media::item_type(item);
        let series = media::series_id(item);
        let series_name = item.get("SeriesName").and_then(Value::as_str);
        tracing::info!(
            item = %current_id,
            item_type,
            series_id = series,
            series_name,
            autoplay = self.config.autoplay,
            has_next = self.window.has_next(),
            "considering series expand"
        );

        // The two directions gate differently: a queue that already has a next
        // item blocks the forward append but is exactly when we want a prepend.
        let forward_reason = series_expand_skip_reason(
            item_type,
            series,
            self.window.has_next(),
            self.config.autoplay,
        );
        let prepend_reason = prepend_skip_reason(item_type, series, self.config.prepend_previous);

        // Fetched for any episode: the playlist selector's titles come from it
        // even when neither direction changes the queue.
        let (Some(series), Some("Episode")) = (series, item_type) else {
            tracing::info!(
                forward = forward_reason,
                prepend = prepend_reason,
                "skipping series expand"
            );
            return;
        };

        tracing::info!(series, start = %current_id, "fetching series episodes");
        let listing = match self.api.episodes_all(series).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("could not list series episodes: {e:#}");
                return;
            }
        };
        self.titles.extend(media::episode_titles(&listing));

        let Some((previous, rest)) = self.split_listing(&listing, current_id) else {
            return;
        };
        self.append_remaining(rest, forward_reason, current_id);
        if prepend_reason.is_none() {
            self.prepend_missing(previous);
        }
    }

    /// Splits the listing at the current episode, or `None` when it is not in
    /// there (specials, library churn, over the 500-episode cap) — fail closed.
    fn split_listing(
        &self,
        listing: &Value,
        current_id: &str,
    ) -> Option<(Vec<String>, Vec<String>)> {
        let items = listing.get("Items").and_then(Value::as_array);
        let listed = items.map(|a| a.len()).unwrap_or(0);
        let current_in_listing = items.is_some_and(|a| {
            a.iter()
                .any(|it| it.get("Id").and_then(Value::as_str) == Some(current_id))
        });
        let (previous, rest) = split_episode_ids(listing, current_id);
        let total = listing.get("TotalRecordCount").and_then(Value::as_i64);
        tracing::info!(
            listed,
            total,
            previous = previous.len(),
            remaining = rest.len(),
            current_in_listing,
            "episodes listing"
        );
        if !current_in_listing {
            tracing::info!(
                current = %current_id,
                listed,
                "current episode not in series listing; not expanding"
            );
            return None;
        }
        Some((previous, rest))
    }

    fn append_remaining(
        &mut self,
        rest: Vec<String>,
        skip_reason: Option<&'static str>,
        current_id: &str,
    ) {
        if let Some(reason) = skip_reason {
            tracing::debug!(reason, "skipping forward append");
        } else if rest.is_empty() {
            tracing::info!(current = %current_id, "no remaining episodes to append");
        } else {
            tracing::info!(n = rest.len(), "queued remaining episodes");
            self.window.append(rest);
            self.log_queue("after-series-expand");
        }
    }

    fn prepend_missing(&mut self, previous: Vec<String>) {
        // Advancing e6 -> e7 leaves e1..e6 already queued ahead of e7.
        let missing = ids_missing_from(&previous, self.window.items());
        if missing.is_empty() {
            tracing::debug!("previous episodes already in queue");
        } else {
            self.prepend_previous_episodes(missing);
        }
    }

    /// Splices already-aired episodes into the queue so the playlist selector
    /// can reach them. Queue only — [`Self::fill_previous_into_mpv`] does the
    /// mpv side later, because `loadfile ... replace` would wipe it.
    fn prepend_previous_episodes(&mut self, previous: Vec<String>) {
        let n = self.window.prepend(previous);
        tracing::info!(n, head = self.window.head(), "prepended previous episodes");
        self.log_queue("after-prepend-previous");
    }

    /// The one place a [`PreparedPlay`] is produced, so also the one place the
    /// remembered tracks are applied.
    pub(super) async fn prepare_item(
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

    /// Splices the pending previous episodes into mpv's playlist. Called once
    /// the current file is loaded, since `loadfile ... replace` would wipe them.
    pub(super) async fn fill_previous_into_mpv(&mut self) {
        let ids = self.window.take_pending_prepend();
        // The whole block at 0 lands in aired order at the front.
        self.load_stub_rows(ids, Fill::Prepend).await;
    }

    /// Appends queue entries past the current mpv window. Titles come from the
    /// series listing; `PlaybackInfo` waits until the item actually plays.
    pub(super) async fn fill_forward_into_mpv(&mut self) {
        let ids: Vec<String> = self.window.forward_ids().to_vec();
        self.load_stub_rows(ids, Fill::Append).await;
    }

    /// Splices `ids` into mpv's playlist right after the current item, at the
    /// mpv position `PlaylistWindow::insert_next` returned. `PlayNext`'s mpv
    /// side: unlike the prepend, there is no upcoming `loadfile ... replace`
    /// to wait out, so this runs immediately instead of being deferred.
    pub(super) async fn insert_next_into_mpv(&mut self, ids: Vec<String>, mpv_pos: usize) {
        self.load_stub_rows(ids, Fill::Next(mpv_pos)).await;
    }

    /// One `loadlist` of stub rows. No HTTP: the titles are already cached and
    /// the URLs are stubs until the row is actually played.
    async fn load_stub_rows(&mut self, ids: Vec<String>, fill: Fill) {
        if ids.is_empty() || self.mpv.is_none() {
            return;
        }
        let n = ids.len();
        tracing::debug!(
            n,
            ?fill,
            origin = self.window.origin(),
            head = self.window.head(),
            tail = self.window.tail(),
            "filling mpv playlist"
        );
        let entries = self.playlist_stub_entries(&ids);
        let refs: Vec<(&str, &str)> = entries
            .iter()
            .map(|(title, url)| (title.as_str(), url.as_str()))
            .collect();
        let Some(mpv) = self.mpv.as_mut() else {
            return;
        };
        let loaded = match fill {
            Fill::Append => mpv.loadlist_append(&refs).await,
            Fill::Prepend => mpv.loadlist_insert_at(&refs, 0).await,
            Fill::Next(index) => mpv.loadlist_insert_at(&refs, index).await,
        };
        if let Err(e) = loaded {
            tracing::warn!(?fill, "playlist fill loadlist: {e:#}");
            return;
        }
        // A prepend or a play-next insert already grew `head`/`tail` when the
        // ids entered the queue, in `PlaylistWindow::prepend` /
        // `PlaylistWindow::insert_next`.
        if fill == Fill::Append {
            self.window.note_appended(n);
        }
        tracing::debug!(n, ?fill, tail = self.window.tail(), "filled mpv playlist");
    }

    fn playlist_stub_entries(&self, ids: &[String]) -> Vec<(String, String)> {
        let token = (!self.mpv_auth_header_set).then_some(self.api.token.as_str());
        ids.iter()
            .map(|id| {
                playlist_stub_entry(
                    &self.api.server,
                    id,
                    self.titles.get(id).map(String::as_str),
                    token,
                )
            })
            .collect()
    }
}

/// `(title, url)` for one playlist row. `token` is `Some` only when the
/// Authorization header is not covering mpv, since mpv persists playlist
/// entries to watch_later files; the title fallback never carries it at all.
fn playlist_stub_entry(
    server: &str,
    id: &str,
    title: Option<&str>,
    token: Option<&str>,
) -> (String, String) {
    let url = jellysink_core::jellyfin::url::direct_stream_url(server, id, id, None, token);
    let title = title.map(str::to_string).unwrap_or_else(|| {
        jellysink_core::jellyfin::url::direct_stream_url(server, id, id, None, None)
    });
    (title, url)
}

/// `(previous, remaining)` around `current_id`. Empty on both sides when the
/// listing does not contain it — fail closed on specials / library churn.
pub(super) fn split_episode_ids(episodes: &Value, current_id: &str) -> (Vec<String>, Vec<String>) {
    let Some(items) = episodes.get("Items").and_then(Value::as_array) else {
        return (Vec::new(), Vec::new());
    };
    let ids: Vec<String> = items
        .iter()
        .filter_map(|it| it.get("Id").and_then(Value::as_str).map(str::to_string))
        .collect();
    match ids.iter().position(|id| id == current_id) {
        Some(i) => {
            let (before, after) = ids.split_at(i);
            (before.to_vec(), after[1..].to_vec())
        }
        None => (Vec::new(), Vec::new()),
    }
}

/// Whether this item could have previous episodes worth prepending. Ignores
/// `has_next` and `autoplay`, unlike [`series_expand_skip_reason`]: both are
/// about continuing forward, not what the playlist selector can reach.
pub(super) fn prepend_skip_reason(
    item_type: Option<&str>,
    series_id: Option<&str>,
    prepend_previous: bool,
) -> Option<&'static str> {
    if !prepend_previous {
        return Some("prepend_previous disabled");
    }
    if item_type != Some("Episode") {
        return Some("item is not an episode");
    }
    if series_id.is_none() {
        return Some("item has no SeriesId");
    }
    None
}

/// Keeps a re-run of the prepend from queueing the same episodes twice.
pub(super) fn ids_missing_from(ids: &[String], queue: &[String]) -> Vec<String> {
    let present: HashSet<&str> = queue.iter().map(String::as_str).collect();
    ids.iter()
        .filter(|id| !present.contains(id.as_str()))
        .cloned()
        .collect()
}

pub(super) fn series_expand_skip_reason(
    item_type: Option<&str>,
    series_id: Option<&str>,
    has_next: bool,
    autoplay: bool,
) -> Option<&'static str> {
    if !autoplay {
        return Some("autoplay disabled");
    }
    if has_next {
        return Some("queue already has a next item");
    }
    if item_type != Some("Episode") {
        return Some("item is not an episode");
    }
    if series_id.is_none() {
        return Some("item has no SeriesId");
    }
    None
}

#[cfg(test)]
#[path = "queue_test.rs"]
mod tests;
