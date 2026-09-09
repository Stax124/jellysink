use super::*;
use serde::Deserialize;
use serde_json::json;

fn item(kind: &str, id: &str, series_id: Option<&str>) -> Item {
    let mut raw = json!({"Id": id, "Name": id, "Type": kind});
    if let Some(series_id) = series_id {
        raw["SeriesId"] = json!(series_id);
    }
    Item::deserialize(raw).unwrap()
}

fn level_of(count: usize) -> Level {
    let mut level = Level::loading("Shows", Source::Libraries);
    level.fill(
        (0..count)
            .map(|i| item("Episode", &i.to_string(), None))
            .collect(),
    );
    level
}

#[test]
fn the_cursor_stops_at_both_ends_rather_than_wrapping_or_overflowing() {
    let mut level = level_of(3);
    level.move_by(-1);
    assert_eq!(level.selected, 0, "already at the top");
    level.move_by(10);
    assert_eq!(level.selected, 2, "clamped to the last row");
    level.move_to_end(End::Top);
    assert_eq!(level.selected, 0);
    level.move_to_end(End::Bottom);
    assert_eq!(level.selected, 2);
}

#[test]
fn an_empty_level_has_nowhere_to_move_and_does_not_panic() {
    let mut level = level_of(0);
    level.move_by(5);
    level.move_to_end(End::Bottom);
    assert_eq!(level.selected, 0);
}

#[test]
fn a_reload_that_returns_fewer_rows_pulls_the_cursor_back_into_range() {
    let mut level = level_of(10);
    level.move_to_end(End::Bottom);
    assert_eq!(level.selected, 9);
    level.fill(vec![item("Episode", "only", None)]);
    assert_eq!(level.selected, 0, "a stale index would index out of bounds");
}

#[test]
fn descend_routes_each_kind_to_the_listing_that_holds_its_children() {
    assert_eq!(
        descend(&item("Series", "s1", None)),
        Some(Source::Seasons("s1".into()))
    );
    assert_eq!(
        descend(&item("Season", "n1", Some("s1"))),
        Some(Source::Episodes {
            series_id: "s1".into(),
            season_id: "n1".into()
        })
    );
    assert_eq!(
        descend(&item("CollectionFolder", "lib", None)),
        Some(Source::Folder("lib".into()))
    );
}

#[test]
fn a_playable_item_does_not_descend() {
    assert_eq!(descend(&item("Episode", "e1", Some("s1"))), None);
    assert_eq!(descend(&item("Movie", "m1", None)), None);
}

#[test]
fn a_season_the_server_sent_no_series_id_for_does_not_descend_into_nothing() {
    assert_eq!(descend(&item("Season", "n1", None)), None);
}
