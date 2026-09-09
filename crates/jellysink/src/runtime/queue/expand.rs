//! Growing the queue from the playing item's series: which episodes come
//! before it, which after, and when neither is worth fetching.

use crate::media;
use crate::runtime::state::Runtime;
use serde_json::Value;
use std::collections::HashSet;

impl Runtime {
    pub(super) async fn try_expand_from_playing_item(&mut self) {
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

    pub(in crate::runtime) async fn maybe_expand_series(&mut self, item: &Value, current_id: &str) {
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
}

/// `(previous, remaining)` around `current_id`. Empty on both sides when the
/// listing does not contain it — fail closed on specials / library churn.
pub(in crate::runtime) fn split_episode_ids(
    episodes: &Value,
    current_id: &str,
) -> (Vec<String>, Vec<String>) {
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
pub(in crate::runtime) fn prepend_skip_reason(
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
pub(in crate::runtime) fn ids_missing_from(ids: &[String], queue: &[String]) -> Vec<String> {
    let present: HashSet<&str> = queue.iter().map(String::as_str).collect();
    ids.iter()
        .filter(|id| !present.contains(id.as_str()))
        .cloned()
        .collect()
}

pub(in crate::runtime) fn series_expand_skip_reason(
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
#[path = "expand_test.rs"]
mod tests;
