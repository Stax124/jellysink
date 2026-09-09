use super::*;
use serde_json::json;

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
