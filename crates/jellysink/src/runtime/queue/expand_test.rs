use super::*;
use serde_json::json;

fn episodes_json(ids: &[&str]) -> Value {
    json!({
        "Items": ids.iter().map(|id| json!({"Id": id})).collect::<Vec<_>>()
    })
}

fn owned(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|id| id.to_string()).collect()
}

#[test]
fn split_episodes_returns_previous_and_remaining() {
    let v = episodes_json(&["e1", "e2", "e3", "e4"]);
    assert_eq!(
        split_episode_ids(&v, "e3"),
        Some((owned(&["e1", "e2"]), owned(&["e4"])))
    );
}

#[test]
fn split_episodes_at_either_end_leaves_that_side_empty() {
    let v = episodes_json(&["e1", "e2"]);
    assert_eq!(split_episode_ids(&v, "e1"), Some((vec![], owned(&["e2"]))));
    assert_eq!(split_episode_ids(&v, "e2"), Some((owned(&["e1"]), vec![])));
}

#[test]
fn split_episodes_fails_closed_when_current_is_missing() {
    let v = episodes_json(&["e1", "e2"]);
    assert_eq!(split_episode_ids(&v, "special"), None);
}

#[test]
fn split_episodes_fails_closed_on_a_malformed_payload() {
    assert_eq!(split_episode_ids(&json!({}), "e1"), None);
    assert_eq!(split_episode_ids(&json!({"Items": "nope"}), "e1"), None);
}

#[test]
fn prepend_runs_when_the_queue_already_has_a_next_item() {
    // The bug: Jellyfin sends 6..20, so has_next is true and the forward
    // gate bails. Prepending must not share that gate.
    assert_eq!(expansion_directions(true, true, true), (false, true));
}

#[test]
fn each_direction_respects_its_own_toggle() {
    assert_eq!(expansion_directions(true, true, false), (true, true));
    assert_eq!(expansion_directions(false, true, false), (false, true));
    assert_eq!(expansion_directions(true, false, false), (true, false));
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
