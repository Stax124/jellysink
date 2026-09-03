use super::Runtime;
use crate::jellyfin::auth::Api;
use crate::media::{self, PlayRequest, PreparedPlay};
use crate::runtime::window::{PlaylistEof, playlist_eof};
use serde_json::Value;
use std::collections::HashSet;

/// Which end of mpv's playlist a batch of stub rows goes on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fill {
    /// After everything mpv already holds.
    Append,
    /// At position 0. Does not interrupt playback; mpv shifts `playlist-pos`.
    Prepend,
}

async fn fetch_prepared(
    api: &Api,
    item_id: &str,
    req: &PlayRequest,
) -> color_eyre::Result<(PreparedPlay, Option<Value>)> {
    let info_fut = api.playback_info(item_id, req);
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
                // Guessing puts the wrong episode on screen. mpv failing to
                // answer means it is gone or wedged, so report a stop instead
                // of autoplaying blind.
                tracing::error!("cannot read mpv playlist state: {e:#}; stopping");
                self.stop_playback(true).await;
                return;
            }
        };
        // The current item's mpv position is its offset from the window start.
        // This stays correct after a prepend (index and head both shift) and
        // after `adopt_playlist_pos` moves the index on a playlist jump.
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
            // The queue has the next item but mpv does not.
            PlaylistEof::NextNotInMpv => self.advance_in_queue().await,
            // Neither has anything left — which is exactly when the series may.
            PlaylistEof::Stop => self.expand_then_advance_or_stop().await,
        }
    }

    /// Hands the advance to mpv, which already holds the next entry.
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
            // Nothing will emit file-loaded now, so the flag would stay set and
            // end_file_action would Ignore every later end-file: autoplay dead
            // until the daemon restarts.
            self.transitioning = false;
            tracing::error!("playlist-next failed: {e:#}");
        }
    }

    /// Starts the next queued item, falling back to a series expand.
    async fn advance_in_queue(&mut self) {
        if self.window.advance().is_some() {
            tracing::info!(item = self.window.current(), "advancing to queued next");
            self.start_next_item().await;
            return;
        }
        self.expand_then_advance_or_stop().await;
    }

    /// Last resort: ask the server for more of the series, and stop if there is
    /// genuinely nothing after this.
    async fn expand_then_advance_or_stop(&mut self) {
        tracing::info!("queue exhausted; trying series expand");
        self.try_expand_from_playing_item().await;
        // `advance` already returns None when there is no next item, so it is
        // the whole condition.
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

    /// mpv's `(playlist-pos, playlist-count)`, or `None` when no mpv is running.
    ///
    /// An IPC failure is an error rather than a fabricated `(0, 0)`:
    /// `playlist_eof` decides autoplay from these two numbers.
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

        // The two directions have different gates. Forward expansion must not
        // run when the queue already has a next item (it would duplicate it),
        // but prepending must run *precisely* then: Jellyfin sending 6..20 is
        // exactly when we also want 1..5.
        let forward_reason = series_expand_skip_reason(
            item_type,
            series,
            self.window.has_next(),
            self.config.autoplay,
        );
        let prepend_reason = prepend_skip_reason(item_type, series, self.config.prepend_previous);

        // Fetch the listing whenever this is an episode: titles for the
        // playlist selector come from it, even when neither direction will
        // change the queue.
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

    /// Splits the listing at the current episode, or `None` when the current
    /// episode is not in it — specials, library churn, or a series longer than
    /// the 500-episode cap. Expansion fails closed there rather than guessing.
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
        // Advancing e6 -> e7 leaves e1..e6 already queued ahead of e7, so only
        // splice in what is genuinely missing.
        let missing = ids_missing_from(&previous, self.window.items());
        if missing.is_empty() {
            tracing::debug!("previous episodes already in queue");
        } else {
            self.prepend_previous_episodes(missing);
        }
    }

    /// Splices the episodes that aired before the current one into the queue
    /// so the playlist selector can reach them.
    ///
    /// Only touches the queue; [`Self::fill_previous_into_mpv`] does the mpv
    /// side. The split matters because `start_current` expands the series
    /// *before* it loads the file, and `loadfile ... replace` wipes mpv's
    /// playlist — so anything inserted before the load would be lost.
    fn prepend_previous_episodes(&mut self, previous: Vec<String>) {
        // Callers pass only ids missing from the queue, so this is idempotent
        // even when it runs again after advancing to the next episode.
        let n = self.window.prepend(previous);
        tracing::info!(n, head = self.window.head(), "prepended previous episodes");
        self.log_queue("after-prepend-previous");
    }

    pub(super) async fn prepare_item(
        &mut self,
        item_id: &str,
        req: &PlayRequest,
    ) -> color_eyre::Result<(PreparedPlay, Option<Value>)> {
        if req.is_plain()
            && let Some(prep) = self.prepared.get(item_id).cloned()
        {
            return Ok((prep, None));
        }

        let (prep, item) = fetch_prepared(&self.api, item_id, req).await?;
        if let Some(ref v) = item {
            self.titles
                .insert(item_id.to_string(), media::display_title(v));
        }
        Ok((prep, item))
    }

    /// Appends queue entries past the current mpv window. Titles come from
    /// the series listing; `PlaybackInfo` waits until the item actually plays.
    /// Splices the pending previous episodes into mpv's playlist. Called once
    /// the current file is loaded, since `loadfile ... replace` would otherwise
    /// wipe them. Titles come from the series listing; there is no per-item
    /// `PlaybackInfo` here.
    pub(super) async fn fill_previous_into_mpv(&mut self) {
        let ids = self.window.take_pending_prepend();
        // Inserting the whole block at 0 lands it in aired order at the front.
        self.load_stub_rows(ids, Fill::Prepend).await;
    }

    /// Appends queue entries past the current mpv window. Titles come from the
    /// series listing; `PlaybackInfo` waits until the item actually plays.
    pub(super) async fn fill_forward_into_mpv(&mut self) {
        let ids: Vec<String> = self.window.forward_ids().to_vec();
        self.load_stub_rows(ids, Fill::Append).await;
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
        };
        if let Err(e) = loaded {
            tracing::warn!(?fill, "playlist fill loadlist: {e:#}");
            return;
        }
        // Only an append extends the window's tail; a prepend already grew
        // `head` when the ids were spliced into the queue.
        if fill == Fill::Append {
            self.window.note_appended(n);
        }
        tracing::debug!(n, ?fill, tail = self.window.tail(), "filled mpv playlist");
    }

    /// `(title, stub url)` for each id.
    ///
    /// Two things here are deliberate. The token goes on the URL only when mpv
    /// is *not* carrying the Authorization header, because mpv persists
    /// playlist entries to its watch_later files. And a missing title falls
    /// back to the URL *without* the token — the fallback used to be the
    /// playable URL, so an episode the series listing had no title for put the
    /// access token straight into mpv's OSD and playlist selector.
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

/// `(title, url)` for one playlist row.
///
/// `token` is `Some` only when mpv is not carrying the Authorization header;
/// mpv persists playlist entries to its watch_later files, so the token stays
/// off the URL whenever the header already covers it. The title never carries
/// the token: the fallback used to be the playable URL, so an episode the
/// series listing had no title for put the token straight into mpv's OSD.
fn playlist_stub_entry(
    server: &str,
    id: &str,
    title: Option<&str>,
    token: Option<&str>,
) -> (String, String) {
    let url = crate::jellyfin::url::direct_stream_url(server, id, id, None, token);
    let title = title
        .map(str::to_string)
        .unwrap_or_else(|| crate::jellyfin::url::direct_stream_url(server, id, id, None, None));
    (title, url)
}

// --- Queue policy -----------------------------------------------------------
// These decide what goes *in* the queue. They lived in media.rs, but their only
// caller is this file and they are not about media.

/// Splits a full series listing into the ids before and after `current_id`.
///
/// Returns `(previous, remaining)`. Empty on both sides when `current_id` is
/// not in the listing — fail closed on specials / library churn.
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

/// Whether this item could have previous episodes worth prepending.
///
/// Deliberately ignores `has_next`, unlike [`series_expand_skip_reason`]:
/// Jellyfin sending 6..20 is exactly when we also want 1..5. Also ignores
/// `autoplay`, which governs continuing *forward*, not what the playlist
/// selector can reach.
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

/// The subset of `ids` not already present in `queue`.
///
/// Keeps prepending idempotent: advancing e6 -> e7 leaves e1..e6 already in
/// the queue ahead of e7, so re-running the expand must not add them twice.
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
mod tests {
    use super::*;
    use serde_json::json;

    #[test]

    fn a_stub_row_with_a_known_title_never_carries_the_token_in_the_title() {
        let (title, url) = playlist_stub_entry("http://s", "e1", Some("Show - s1e01"), Some("tok"));
        assert_eq!(title, "Show - s1e01");
        assert!(url.contains("ApiKey=tok"));
    }

    /// The leak this guards: the fallback used to be the playable URL, so an
    /// episode missing from the series listing showed the access token in mpv's
    /// playlist selector and wrote it to the user's watch_later files.
    #[test]
    fn a_stub_row_without_a_title_falls_back_to_a_tokenless_url() {
        let (title, url) = playlist_stub_entry("http://s", "e1", None, Some("tok"));
        assert!(!title.contains("ApiKey"), "title leaked the token: {title}");
        assert!(title.contains("e1"), "title should still identify the row");
        assert!(
            url.contains("ApiKey=tok"),
            "the playable url still needs auth"
        );
    }

    #[test]

    fn no_token_goes_on_the_url_when_mpv_carries_the_auth_header() {
        let (_, url) = playlist_stub_entry("http://s", "e1", Some("t"), None);
        assert!(!url.contains("ApiKey"), "{url}");
    }

    fn episodes_json(ids: &[&str]) -> Value {
        json!({
            "Items": ids.iter().map(|id| json!({"Id": id})).collect::<Vec<_>>()
        })
    }

    #[test]

    fn split_episodes_returns_previous_and_remaining() {
        let v = episodes_json(&["e1", "e2", "e3", "e4"]);
        let (previous, remaining) = split_episode_ids(&v, "e3");
        assert_eq!(previous, vec!["e1".to_string(), "e2".to_string()]);
        assert_eq!(remaining, vec!["e4".to_string()]);
    }

    #[test]

    fn split_episodes_at_the_first_has_no_previous() {
        let v = episodes_json(&["e1", "e2"]);
        let (previous, remaining) = split_episode_ids(&v, "e1");
        assert!(previous.is_empty());
        assert_eq!(remaining, vec!["e2".to_string()]);
    }

    #[test]

    fn split_episodes_at_the_last_has_no_remaining() {
        let v = episodes_json(&["e1", "e2"]);
        let (previous, remaining) = split_episode_ids(&v, "e2");
        assert_eq!(previous, vec!["e1".to_string()]);
        assert!(remaining.is_empty());
    }

    #[test]

    fn split_episodes_empty_when_current_is_missing() {
        let v = episodes_json(&["e1", "e2"]);
        assert_eq!(split_episode_ids(&v, "special"), (vec![], vec![]));
    }

    #[test]

    fn split_episodes_empty_on_malformed_payload() {
        assert_eq!(split_episode_ids(&json!({}), "e1"), (vec![], vec![]));
        assert_eq!(
            split_episode_ids(&json!({"Items": "nope"}), "e1"),
            (vec![], vec![])
        );
    }

    #[test]

    fn prepend_runs_when_the_queue_already_has_a_next_item() {
        // The bug: Jellyfin sends 6..20, so has_next is true and the forward
        // gate bails. Prepending must not share that gate.
        assert_eq!(
            prepend_skip_reason(Some("Episode"), Some("series-1"), true),
            None
        );
        assert_eq!(
            series_expand_skip_reason(Some("Episode"), Some("series-1"), true, true),
            Some("queue already has a next item")
        );
    }

    #[test]

    fn prepend_ignores_autoplay() {
        // autoplay governs continuing forward, not what the selector reaches.
        assert_eq!(
            prepend_skip_reason(Some("Episode"), Some("series-1"), true),
            None
        );
    }

    #[test]

    fn prepend_respects_its_own_toggle() {
        assert_eq!(
            prepend_skip_reason(Some("Episode"), Some("series-1"), false),
            Some("prepend_previous disabled")
        );
    }

    #[test]

    fn prepend_skips_non_episodes_and_seriesless_items() {
        assert_eq!(
            prepend_skip_reason(Some("Movie"), Some("series-1"), true),
            Some("item is not an episode")
        );
        assert_eq!(
            prepend_skip_reason(Some("Episode"), None, true),
            Some("item has no SeriesId")
        );
    }

    #[test]

    fn ids_missing_from_drops_what_the_queue_already_has() {
        let previous = ["e1".to_string(), "e2".to_string(), "e3".to_string()];
        let queue = ["e1".to_string(), "e2".to_string(), "e4".to_string()];
        assert_eq!(ids_missing_from(&previous, &queue), vec!["e3".to_string()]);
    }

    #[test]

    fn ids_missing_from_is_empty_when_all_present() {
        let previous = ["e1".to_string(), "e2".to_string()];
        let queue = ["e1".to_string(), "e2".to_string(), "e3".to_string()];
        assert!(ids_missing_from(&previous, &queue).is_empty());
    }

    #[test]

    fn ids_missing_from_keeps_order() {
        let previous = ["e3".to_string(), "e1".to_string(), "e2".to_string()];
        assert_eq!(
            ids_missing_from(&previous, &[]),
            vec!["e3".to_string(), "e1".to_string(), "e2".to_string()]
        );
    }

    #[test]

    fn expand_series_only_for_a_lonely_episode() {
        assert_eq!(
            series_expand_skip_reason(Some("Episode"), Some("series-1"), false, true),
            None
        );
        assert_eq!(
            series_expand_skip_reason(Some("Movie"), Some("series-1"), false, true),
            Some("item is not an episode")
        );
        assert_eq!(
            series_expand_skip_reason(Some("Episode"), None, false, true),
            Some("item has no SeriesId")
        );
        assert_eq!(
            series_expand_skip_reason(Some("Episode"), Some("series-1"), true, true),
            Some("queue already has a next item")
        );
        assert_eq!(
            series_expand_skip_reason(Some("Episode"), Some("series-1"), false, false),
            Some("autoplay disabled")
        );
    }
}
