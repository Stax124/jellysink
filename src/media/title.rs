//! Display titles: what the user sees in mpv and in the playlist selector.
use serde_json::Value;
use std::collections::HashMap;

/// Window / OSC title, matching jellyfin-mpv-shim's `get_proper_title`.
pub(crate) fn display_title(item: &Value) -> String {
    let name = item
        .get("Name")
        .and_then(Value::as_str)
        .unwrap_or("Jellyfin");
    let is_episode = item.get("Type").and_then(Value::as_str) == Some("Episode");
    if is_episode
        && let (Some(series), Some(season), Some(episode)) = (
            item.get("SeriesName").and_then(Value::as_str),
            item.get("ParentIndexNumber").and_then(Value::as_i64),
            item.get("IndexNumber").and_then(Value::as_i64),
        )
    {
        return format!("{series} - s{season}e{episode:02} - {name}");
    }
    if item.get("Type").and_then(Value::as_str) == Some("Movie")
        && let Some(year) = item.get("ProductionYear").and_then(Value::as_i64)
    {
        return format!("{name} ({year})");
    }
    name.to_string()
}

/// Display titles from a series episode listing, keyed by item id.
///
/// Playlist fill uses this so mpv's selector has names without a per-item
/// `PlaybackInfo` / `GET /Items/{id}` round trip.
pub(crate) fn episode_titles(listing: &Value) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Some(items) = listing.get("Items").and_then(Value::as_array) else {
        return out;
    };
    for it in items {
        let Some(id) = it.get("Id").and_then(Value::as_str) else {
            continue;
        };
        out.insert(id.to_string(), display_title(it));
    }
    out
}

pub(crate) fn item_type(item: &Value) -> Option<&str> {
    item.get("Type").and_then(Value::as_str)
}

pub(crate) fn series_id(item: &Value) -> Option<&str> {
    item.get("SeriesId").and_then(Value::as_str)
}

#[cfg(test)]
#[path = "title_test.rs"]
mod tests;
