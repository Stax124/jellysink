use super::Runtime;
use crate::cast::{PlaylistEof, playlist_eof};
use crate::jellyfin::auth::Api;
use crate::jellyfin::playback::PlaybackEndpoints;
use crate::media::{self, PreparedPlay};
use serde_json::Value;

async fn fetch_prepared(
    api: &Api,
    item_id: &str,
    start_ticks: Option<i64>,
    aid: Option<i64>,
    sid: Option<i64>,
    srcid: Option<&str>,
) -> color_eyre::Result<(PreparedPlay, Option<Value>)> {
    let endpoints = PlaybackEndpoints::new(api);
    let info_fut = endpoints.playback_info(item_id, start_ticks, aid, sid, srcid);
    let item_fut = endpoints.get_item(item_id);
    let (info, item) = tokio::join!(info_fut, item_fut);
    let info = info?;
    let item = match item {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::debug!("could not fetch item metadata: {e:#}");
            None
        }
    };
    let mut prep = media::prepare_play(&api.server, item_id, &info, srcid, aid, sid, &api.token)?;
    if let Some(ref v) = item {
        prep.title = media::display_title(v);
    }
    Ok((prep, item))
}

impl Runtime {
    pub(super) fn log_queue(&self, at: &str) {
        tracing::info!(
            at,
            index = self.queue.index,
            queue = self.queue.items.len(),
            current = self.queue.current(),
            next = self.queue.peek_next(),
            autoplay = self.config.autoplay,
            "queue"
        );
    }

    pub(super) async fn play_next_or_stop(&mut self, from_eof: bool) {
        self.log_queue("play-next-or-stop");
        let pos = if let Some(mpv) = self.mpv.as_mut() {
            mpv.playlist_pos().await.unwrap_or(0).max(0) as usize
        } else {
            0
        };
        let count = if let Some(mpv) = self.mpv.as_mut() {
            mpv.playlist_count().await.unwrap_or(0).max(0) as usize
        } else {
            0
        };
        // The current item's mpv position is its offset from the window start.
        // This stays correct after a prepend (index and head both shift) and
        // after `adopt_playlist_pos` moves the index on a playlist jump.
        let expected_pos = self.queue.index.saturating_sub(self.playlist_origin);
        tracing::info!(
            pos,
            count,
            expected_pos,
            head = self.mpv_head,
            from_eof,
            has_next = self.queue.has_next(),
            "eof playlist state"
        );
        match playlist_eof(pos, count, self.queue.has_next(), expected_pos, from_eof) {
            PlaylistEof::NextInMpv => {
                tracing::info!(pos, count, expected_pos, "playlist-next");
                self.transitioning = true;
                if let Some(mpv) = self.mpv.as_mut() {
                    let _ = mpv.playlist_next().await;
                    let _ = mpv.unpause().await;
                }
            }
            PlaylistEof::WaitForMpv => {
                tracing::info!(
                    pos,
                    count,
                    expected_pos,
                    "eof; mpv will play next (waiting for file-loaded)"
                );
            }
            PlaylistEof::NextNotInMpv => {
                if self.queue.advance().is_some() {
                    tracing::info!(item = self.queue.current(), "advancing to queued next");
                    if let Err(e) = self.start_current(None, None, None, None).await {
                        tracing::error!("next item failed: {e:#}");
                    }
                } else {
                    tracing::info!("queue exhausted; trying series expand");
                    self.try_expand_from_playing_item().await;
                    if self.queue.advance().is_some() {
                        tracing::info!(
                            item = self.queue.current(),
                            "advancing after series expand"
                        );
                        if let Err(e) = self.start_current(None, None, None, None).await {
                            tracing::error!("next item failed: {e:#}");
                        }
                    } else {
                        tracing::info!("no next episode; stopping");
                        self.stop_playback(true).await;
                    }
                }
            }
            PlaylistEof::Stop => {
                tracing::info!("queue exhausted; trying series expand");
                self.try_expand_from_playing_item().await;
                if self.queue.has_next() && self.queue.advance().is_some() {
                    tracing::info!(item = self.queue.current(), "advancing after series expand");
                    if let Err(e) = self.start_current(None, None, None, None).await {
                        tracing::error!("next item failed: {e:#}");
                    }
                } else {
                    tracing::info!("no next episode; stopping");
                    self.stop_playback(true).await;
                }
            }
        }
    }

    async fn try_expand_from_playing_item(&mut self) {
        let Some(item_id) = self.item_id.clone() else {
            return;
        };
        let endpoints = self.playback();
        let item = match endpoints.get_item(&item_id).await {
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
            has_next = self.queue.has_next(),
            "considering series expand"
        );
        // The two directions have different gates. Forward expansion must not
        // run when the queue already has a next item (it would duplicate it),
        // but prepending must run *precisely* then: Jellyfin sending 6..20 is
        // exactly when we also want 1..5.
        let forward_reason = media::series_expand_skip_reason(
            item_type,
            series,
            self.queue.has_next(),
            self.config.autoplay,
        );
        let prepend_reason =
            media::prepend_skip_reason(item_type, series, self.config.prepend_previous);
        // Fetch the listing whenever this is an episode: titles for the
        // playlist selector come from it, even if neither direction will
        // change the queue.
        let Some(series) = series else {
            tracing::info!(
                forward = forward_reason,
                prepend = prepend_reason,
                "skipping series expand"
            );
            return;
        };
        if item_type != Some("Episode") {
            tracing::info!(
                forward = forward_reason,
                prepend = prepend_reason,
                "skipping series expand"
            );
            return;
        }
        let endpoints = self.playback();
        tracing::info!(series, start = %current_id, "fetching series episodes");
        match endpoints.episodes_all(series).await {
            Ok(v) => {
                self.titles.extend(media::episode_titles(&v));
                let items = v.get("Items").and_then(Value::as_array);
                let listed = items.map(|a| a.len()).unwrap_or(0);
                let total = v.get("TotalRecordCount").and_then(Value::as_i64);
                let current_in_listing = items.is_some_and(|a| {
                    a.iter()
                        .any(|it| it.get("Id").and_then(Value::as_str) == Some(current_id))
                });
                let (previous, rest) = media::split_episode_ids(&v, current_id);
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
                    return;
                }
                match forward_reason {
                    None => {
                        if rest.is_empty() {
                            tracing::info!(
                                current = %current_id,
                                listed,
                                "no remaining episodes to append"
                            );
                        } else {
                            tracing::info!(n = rest.len(), "queued remaining episodes");
                            self.queue.append(rest);
                            self.log_queue("after-series-expand");
                        }
                    }
                    Some(reason) => {
                        tracing::debug!(reason, "skipping forward append");
                    }
                }
                if prepend_reason.is_none() && !previous.is_empty() {
                    // Advancing e6 -> e7 leaves e1..e6 already queued ahead of
                    // e7, so only splice in what is genuinely missing.
                    let missing = media::ids_missing_from(&previous, &self.queue.items);
                    if missing.is_empty() {
                        tracing::debug!("previous episodes already in queue");
                    } else {
                        self.prepend_previous_episodes(missing);
                    }
                }
            }
            Err(e) => tracing::warn!("could not list series episodes: {e:#}"),
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
        // mpv's playlist is the contiguous window
        //   items[origin .. origin + mpv_head + 1 + mpv_tail]
        // and the current item sits at mpv position `mpv_head`. Prepending
        // shifts the current item's queue index and its mpv position by the
        // same amount, so `origin` is unchanged — only `mpv_head` grows.
        let n = self.queue.insert_before_current(previous.clone());
        self.mpv_head += n;
        self.pending_prepend = previous;
        tracing::info!(n, head = self.mpv_head, "prepended previous episodes");
        self.log_queue("after-prepend-previous");
    }

    /// Splices the pending previous episodes into mpv's playlist. Called once
    /// the current file is loaded, since `loadfile ... replace` would
    /// otherwise wipe them. Titles come from the series listing; there is no
    /// per-item `PlaybackInfo` here.
    pub(super) async fn fill_previous_into_mpv(&mut self) {
        let ids = std::mem::take(&mut self.pending_prepend);
        if ids.is_empty() || self.mpv.is_none() {
            return;
        }
        let n = ids.len();
        let entries = self.playlist_stub_entries(&ids);
        let refs: Vec<(&str, &str)> = entries
            .iter()
            .map(|(title, url)| (title.as_str(), url.as_str()))
            .collect();
        // Inserting the whole block at 0 lands it in aired order at the front.
        let Some(mpv) = self.mpv.as_mut() else {
            return;
        };
        if let Err(e) = mpv.loadlist_insert_at(&refs, 0).await {
            tracing::warn!("prepend loadlist: {e:#}");
            return;
        }
        tracing::debug!(n, "inserted previous episodes into mpv");
    }

    pub(super) async fn prepare_item(
        &mut self,
        item_id: &str,
        start_ticks: Option<i64>,
        aid: Option<i64>,
        sid: Option<i64>,
        srcid: Option<&str>,
    ) -> color_eyre::Result<(PreparedPlay, Option<Value>)> {
        if aid.is_none()
            && sid.is_none()
            && srcid.is_none()
            && start_ticks.unwrap_or(0) == 0
            && let Some(prep) = self.prepared.get(item_id).cloned()
        {
            return Ok((prep, None));
        }

        let (prep, item) = fetch_prepared(&self.api, item_id, start_ticks, aid, sid, srcid).await?;
        if let Some(ref v) = item {
            self.titles
                .insert(item_id.to_string(), media::display_title(v));
        }
        Ok((prep, item))
    }

    /// Appends queue entries past the current mpv window. Titles come from
    /// the series listing; `PlaybackInfo` waits until the item actually plays.
    pub(super) async fn fill_forward_into_mpv(&mut self) {
        if self.mpv.is_none() {
            return;
        }
        let origin = self.playlist_origin;
        let ids: Vec<String> = self
            .queue
            .items
            .get(origin + self.mpv_head + 1 + self.mpv_tail..)
            .unwrap_or(&[])
            .to_vec();
        if ids.is_empty() {
            return;
        }
        tracing::debug!(
            n = ids.len(),
            origin,
            head = self.mpv_head,
            tail = self.mpv_tail,
            "filling mpv playlist"
        );
        let n = ids.len();
        let entries = self.playlist_stub_entries(&ids);
        let refs: Vec<(&str, &str)> = entries
            .iter()
            .map(|(title, url)| (title.as_str(), url.as_str()))
            .collect();
        let Some(mpv) = self.mpv.as_mut() else {
            return;
        };
        if let Err(e) = mpv.loadlist_append(&refs).await {
            tracing::warn!("playlist fill loadlist: {e:#}");
            return;
        }
        self.mpv_tail += n;
        tracing::debug!(n, tail = self.mpv_tail, "appended to mpv playlist");
    }

    /// `(title, stub url)` for each id. Missing titles fall back to the URL,
    /// which is what the selector showed before M3U titles existed.
    fn playlist_stub_entries(&self, ids: &[String]) -> Vec<(String, String)> {
        ids.iter()
            .map(|id| {
                let url =
                    media::direct_stream_url(&self.api.server, id, id, None, Some(&self.api.token));
                let title = self.titles.get(id).cloned().unwrap_or_else(|| url.clone());
                (title, url)
            })
            .collect()
    }
}

/// The mpv playlist is the contiguous window
/// `items[origin .. origin + head + 1 + tail]`, and the current item sits at
/// mpv position `head`. These tests pin that invariant across a prepend,
/// because every index calculation in this module depends on it.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::cast::Queue;
    use serde_json::json;

    /// Simulates the state after `start_current` plus a prepend, without mpv.
    struct Window {
        queue: Queue,
        origin: usize,
        head: usize,
        tail: usize,
    }

    impl Window {
        /// `start_current`: mpv holds only the current item.
        fn start(items: Vec<&str>, index: usize) -> Self {
            Self {
                queue: Queue {
                    items: items.into_iter().map(str::to_string).collect(),
                    index,
                },
                origin: index,
                head: 0,
                tail: 0,
            }
        }

        /// `prepend_previous_episodes`: splice in, head grows, origin holds.
        fn prepend(&mut self, ids: Vec<&str>) {
            let n = self
                .queue
                .insert_before_current(ids.into_iter().map(str::to_string).collect());
            self.head += n;
        }

        /// The mpv playlist this window claims is loaded.
        fn mpv_playlist(&self) -> Vec<String> {
            self.queue.items[self.origin..self.origin + self.head + 1 + self.tail].to_vec()
        }

        /// The item mpv is playing, per the window's own arithmetic.
        fn current(&self) -> &str {
            &self.queue.items[self.origin + self.mpv_pos()]
        }

        /// The current item's mpv playlist position.
        fn mpv_pos(&self) -> usize {
            self.queue.index - self.origin
        }
    }

    #[test]
    fn prepend_keeps_the_window_contiguous_and_current_stable() {
        // Playing e8 of a 13-episode series; 8..13 already queued.
        let mut w = Window::start(vec!["e8", "e9", "e10", "e11", "e12", "e13"], 0);
        assert_eq!(w.current(), "e8");

        w.prepend(vec!["e1", "e2", "e3", "e4", "e5", "e6", "e7"]);

        assert_eq!(w.current(), "e8", "current item must not move");
        assert_eq!(w.head, 7);
        assert_eq!(w.origin, 0, "origin is unchanged by a prepend");
        assert_eq!(
            w.mpv_playlist(),
            vec!["e1", "e2", "e3", "e4", "e5", "e6", "e7", "e8"]
        );
    }

    #[test]
    fn prepend_then_fill_appends_after_the_current_item() {
        let mut w = Window::start(vec!["e8", "e9", "e10"], 0);
        w.prepend(vec!["e6", "e7"]);
        // Two forward entries land in mpv.
        w.tail = 2;

        assert_eq!(w.current(), "e8");
        assert_eq!(
            w.mpv_playlist(),
            vec!["e6", "e7", "e8", "e9", "e10"],
            "the window must stay contiguous across head and tail"
        );
        assert_eq!(
            w.origin + w.head + 1 + w.tail,
            w.queue.items.len(),
            "queue exhausted"
        );
    }

    #[test]
    fn prepend_from_a_mid_series_start_index() {
        // Jellyfin sent 8..13 with StartIndex=2, so we start on e10.
        let mut w = Window::start(vec!["e8", "e9", "e10", "e11"], 2);
        assert_eq!(w.current(), "e10");
        assert_eq!(w.origin, 2);

        w.prepend(vec!["e1", "e2", "e3", "e4", "e5", "e6", "e7", "e8", "e9"]);

        assert_eq!(w.current(), "e10", "current item must not move");
        assert_eq!(w.origin, 2, "origin is unchanged by a prepend");
        assert_eq!(w.head, 9);
        assert_eq!(
            w.mpv_playlist(),
            vec!["e1", "e2", "e3", "e4", "e5", "e6", "e7", "e8", "e9", "e10"]
        );
    }

    #[test]
    fn prepend_happens_even_though_jellyfin_sent_a_full_queue() {
        // Reproduces the reported bug: Jellyfin sends e6..e20, so has_next is
        // true and the forward gate bails. The prepend must still run, and the
        // forward append must not duplicate e7..e20.
        let mut w = Window::start(vec!["e6", "e7", "e8"], 0);
        assert!(w.queue.has_next(), "forward gate would bail here");

        // Full series listing split at e6.
        let (previous, _rest) = split_at(&["e1", "e2", "e3", "e4", "e5", "e6", "e7"], "e6");
        let missing = media::ids_missing_from(&previous, &w.queue.items);
        assert_eq!(missing, vec!["e1", "e2", "e3", "e4", "e5"]);

        w.prepend(missing.iter().map(String::as_str).collect());

        assert_eq!(w.current(), "e6", "current item must not move");
        assert_eq!(w.origin, 0);
        assert_eq!(w.head, 5);
        assert_eq!(
            w.mpv_playlist(),
            vec!["e1", "e2", "e3", "e4", "e5", "e6"],
            "e7..e20 must not be duplicated by the forward append"
        );
    }

    #[test]
    fn advancing_then_prepending_does_not_duplicate() {
        // After e6 ends we advance to e7; e1..e6 are already queued ahead of
        // it, so re-running the expand must add nothing.
        let mut w = Window::start(vec!["e6", "e7", "e8"], 0);
        w.prepend(vec!["e1", "e2", "e3", "e4", "e5"]);
        w.queue.index += 1; // advance to e7
        assert_eq!(w.current(), "e7");

        let (previous, _rest) = split_at(&["e1", "e2", "e3", "e4", "e5", "e6", "e7"], "e7");
        let missing = media::ids_missing_from(&previous, &w.queue.items);
        assert!(
            missing.is_empty(),
            "e1..e6 are already in the queue: {missing:?}"
        );
    }

    #[test]
    fn expected_pos_follows_a_playlist_jump_not_the_head() {
        // Jumping to e7 via the playlist selector moves the queue index but
        // leaves `head` at 5. Deriving the position from `head` would report
        // 5 and misread every subsequent EOF.
        let mut w = Window::start(vec!["e6", "e7", "e8"], 0);
        w.prepend(vec!["e1", "e2", "e3", "e4", "e5"]);
        assert_eq!(w.head, 5);

        w.queue.index += 1; // adopt_playlist_pos jumps to e7
        assert_eq!(w.current(), "e7");
        assert_eq!(w.mpv_pos(), 6, "position is index - origin, not head");
        assert_ne!(w.mpv_pos(), w.head);
    }

    /// Test helper: split a listing of ids at `current`.
    fn split_at(ids: &[&str], current: &str) -> (Vec<String>, Vec<String>) {
        let v = json!({
            "Items": ids.iter().map(|id| json!({"Id": id})).collect::<Vec<_>>()
        });
        media::split_episode_ids(&v, current)
    }

    #[test]
    fn eof_expected_pos_is_not_zero_when_previous_episodes_are_loaded() {
        // With previous episodes spliced in, the current item is at mpv
        // position 2, not 0; comparing against 0 would misread every EOF.
        let mut w = Window::start(vec!["e8", "e9"], 0);
        w.prepend(vec!["e6", "e7"]);
        w.tail = 1;

        let count = w.mpv_playlist().len();
        let expected_pos = w.mpv_pos();
        assert_eq!(count, 4);
        assert_eq!(expected_pos, 2);
        // mpv already advanced to e9 on EOF: wait rather than playlist-next.
        assert_eq!(
            playlist_eof(3, count, false, expected_pos, true),
            PlaylistEof::WaitForMpv
        );
        // Still on e8 with e9 in mpv: let mpv autoplay it.
        assert_eq!(
            playlist_eof(2, count, true, expected_pos, true),
            PlaylistEof::WaitForMpv
        );
    }
}
