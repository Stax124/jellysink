//! Growing the queue from the playing item's series: which episodes come
//! before it, which after, and when neither is worth fetching.

use crate::media;
use crate::runtime::state::Runtime;
use serde_json::Value;
use std::collections::HashSet;

impl Runtime {
    pub(super) async fn try_expand_from_playing_item(&mut self) {
        let Some(item_id) = self.current.as_ref().map(|current| current.item_id.clone()) else {
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

    pub(in crate::runtime) async fn maybe_expand_series(&mut self, item: &Value, current_id: &str) {
        let item_type = media::item_type(item);
        let series = media::series_id(item);
        let series_name = item.get("SeriesName").and_then(Value::as_str);
        let (forward, prepend) = expansion_directions(
            self.config.autoplay,
            self.config.prepend_previous,
            self.window.has_next(),
        );
        tracing::info!(
            item = %current_id,
            item_type,
            series_id = series,
            series_name,
            autoplay = self.config.autoplay,
            prepend_previous = self.config.prepend_previous,
            has_next = self.window.has_next(),
            forward,
            prepend,
            "considering series expand"
        );

        // Fetched for any episode: the playlist selector's titles come from it
        // even when neither direction changes the queue.
        let (Some(series), Some("Episode")) = (series, item_type) else {
            tracing::info!("not an episode of a series; skipping series expand");
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

        let listed = listing
            .get("Items")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        let total = listing.get("TotalRecordCount").and_then(Value::as_i64);
        let Some((previous, rest)) = split_episode_ids(&listing, current_id) else {
            tracing::info!(
                current = %current_id,
                listed,
                total,
                "current episode not in series listing; not expanding"
            );
            return;
        };
        tracing::info!(
            listed,
            total,
            previous = previous.len(),
            remaining = rest.len(),
            "episodes listing"
        );
        if forward {
            self.append_remaining(rest, current_id);
        }
        if prepend {
            self.prepend_missing(previous);
        }
    }

    fn append_remaining(&mut self, rest: Vec<String>, current_id: &str) {
        if rest.is_empty() {
            tracing::info!(current = %current_id, "no remaining episodes to append");
        } else {
            tracing::info!(n = rest.len(), "queued remaining episodes");
            self.window.append(rest);
            self.log_queue("after-series-expand");
        }
    }

    /// Splices already-aired episodes into the queue only:
    /// [`Self::fill_previous_into_mpv`] follows, since `replace` would wipe it.
    fn prepend_missing(&mut self, previous: Vec<String>) {
        // Advancing e6 -> e7 leaves e1..e6 already queued ahead of e7.
        let missing = ids_missing_from(&previous, self.window.items());
        if missing.is_empty() {
            tracing::debug!("previous episodes already in queue");
            return;
        }
        let n = self.window.prepend(missing);
        tracing::info!(n, head = self.window.head(), "prepended previous episodes");
        self.log_queue("after-prepend-previous");
    }
}

/// `(forward, prepend)`. Separate gates: a queue that already has a next item
/// blocks the forward append but is exactly when the prepend is wanted.
pub(in crate::runtime) fn expansion_directions(
    autoplay: bool,
    prepend_previous: bool,
    has_next: bool,
) -> (bool, bool) {
    (autoplay && !has_next, prepend_previous)
}

/// `(previous, remaining)` around `current_id`, or `None` when the listing
/// does not contain it — fail closed on specials, library churn and the cap.
pub(in crate::runtime) fn split_episode_ids(
    episodes: &Value,
    current_id: &str,
) -> Option<(Vec<String>, Vec<String>)> {
    let ids: Vec<String> = episodes
        .get("Items")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(|it| it.get("Id").and_then(Value::as_str).map(str::to_string))
        .collect();
    let i = ids.iter().position(|id| id == current_id)?;
    let (before, after) = ids.split_at(i);
    Some((before.to_vec(), after[1..].to_vec()))
}

/// Keeps a re-run of the prepend from queueing the same episodes twice.
pub(in crate::runtime) fn ids_missing_from(ids: &[String], queue: &[String]) -> Vec<String> {
    let present: HashSet<&str> = queue.iter().map(String::as_str).collect();
    ids.iter()
        .filter(|id| !present.contains(id.as_str()))
        .cloned()
        .collect()
}

#[cfg(test)]
#[path = "expand_test.rs"]
mod tests;
