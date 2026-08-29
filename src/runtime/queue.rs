use super::{FillMsg, Runtime};
use crate::cast::{PlaylistEof, next_fill_id, playlist_eof};
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
        let expected_pos = self.queue.index.saturating_sub(self.playlist_origin);
        tracing::info!(
            pos,
            count,
            expected_pos,
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
        if let Some(reason) = media::series_expand_skip_reason(
            item_type,
            series,
            self.queue.has_next(),
            self.config.autoplay,
        ) {
            tracing::info!(reason, "skipping series expand");
            return;
        }
        let Some(series) = series else {
            return;
        };
        let endpoints = self.playback();
        tracing::info!(series, start = %current_id, "fetching remaining episodes");
        match endpoints.episodes_from(series, current_id).await {
            Ok(v) => {
                let items = v.get("Items").and_then(Value::as_array);
                let listed = items.map(|a| a.len()).unwrap_or(0);
                let total = v.get("TotalRecordCount").and_then(Value::as_i64);
                let keys = v.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>());
                let first = items
                    .and_then(|a| a.first())
                    .and_then(|it| it.get("Id").and_then(Value::as_str));
                let current_in_listing = items.is_some_and(|a| {
                    a.iter()
                        .any(|it| it.get("Id").and_then(Value::as_str) == Some(current_id))
                });
                let rest = media::remaining_episode_ids(&v, current_id);
                tracing::info!(
                    listed,
                    total,
                    first,
                    remaining = rest.len(),
                    current_in_listing,
                    ?keys,
                    "episodes listing"
                );
                if rest.is_empty() {
                    tracing::info!(
                        current = %current_id,
                        listed,
                        current_in_listing,
                        "no remaining episodes to append"
                    );
                } else {
                    tracing::info!(n = rest.len(), "queued remaining episodes");
                    self.queue.append(rest);
                    self.log_queue("after-series-expand");
                }
            }
            Err(e) => tracing::warn!("could not list remaining episodes: {e:#}"),
        }
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

        fetch_prepared(&self.api, item_id, start_ticks, aid, sid, srcid).await
    }

    pub(super) fn abort_fill(&mut self) {
        if let Some(handle) = self.fill.take() {
            handle.abort();
        }
        self.fill_gen = self.fill_gen.wrapping_add(1);
    }

    pub(super) fn spawn_playlist_fill(&mut self) {
        if self.mpv.is_none() {
            return;
        }
        self.abort_fill();
        let origin = self.playlist_origin;
        let ids: Vec<String> = self
            .queue
            .items
            .get(origin + 1 + self.mpv_tail..)
            .unwrap_or(&[])
            .to_vec();
        if ids.is_empty() {
            return;
        }
        let generation = self.fill_gen;
        let api = self.api.clone();
        let tx = self.fill_tx.clone();
        tracing::debug!(
            n = ids.len(),
            origin,
            tail = self.mpv_tail,
            "filling mpv playlist"
        );
        self.fill = Some(tokio::spawn(async move {
            for item_id in ids {
                match fetch_prepared(&api, &item_id, None, None, None, None).await {
                    Ok((prep, _)) => {
                        if tx
                            .send(FillMsg {
                                generation,
                                item_id,
                                prep,
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(e) => tracing::warn!(item = %item_id, "playlist fill failed: {e:#}"),
                }
            }
        }));
    }

    pub(super) async fn on_fill_ready(&mut self, msg: FillMsg) {
        if msg.generation != self.fill_gen {
            return;
        }
        self.prepared.insert(msg.item_id.clone(), msg.prep.clone());
        while let Some(id) =
            next_fill_id(&self.queue, self.playlist_origin, 1 + self.mpv_tail).map(str::to_string)
        {
            let Some(prep) = self.prepared.get(&id).cloned() else {
                break;
            };
            let Some(mpv) = self.mpv.as_mut() else {
                break;
            };
            let url = if prep.url.contains("ApiKey=") {
                prep.url.clone()
            } else {
                media::direct_stream_url(
                    &self.api.server,
                    &id,
                    &prep.media_source_id,
                    prep.live_stream_id.as_deref(),
                    Some(&self.api.token),
                )
            };
            if let Err(e) = mpv.loadfile_append(&url, &prep.title).await {
                tracing::warn!(item = %id, "loadfile append: {e:#}");
                break;
            }
            self.mpv_tail += 1;
            tracing::debug!(item = %id, tail = self.mpv_tail, "appended to mpv playlist");
        }
    }
}
