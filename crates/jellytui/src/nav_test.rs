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

fn rows_of(count: usize) -> Rows {
    let mut rows = Rows::loading();
    rows.fill(
        (0..count)
            .map(|i| item("Episode", &i.to_string(), None))
            .collect(),
    );
    rows
}

#[test]
fn the_cursor_stops_at_both_ends_rather_than_wrapping_or_overflowing() {
    let mut rows = rows_of(3);
    rows.move_by(-1);
    assert_eq!(rows.selected, 0, "already at the top");
    rows.move_by(10);
    assert_eq!(rows.selected, 2, "clamped to the last row");
    rows.move_to_end(End::Top);
    assert_eq!(rows.selected, 0);
    rows.move_to_end(End::Bottom);
    assert_eq!(rows.selected, 2);
}

#[test]
fn an_empty_list_has_nowhere_to_move_and_does_not_panic() {
    let mut rows = rows_of(0);
    rows.move_by(5);
    rows.move_to_end(End::Bottom);
    assert_eq!(rows.selected, 0);
}

#[test]
fn a_reload_that_returns_fewer_rows_pulls_the_cursor_back_into_range() {
    let mut rows = rows_of(10);
    rows.move_to_end(End::Bottom);
    assert_eq!(rows.selected, 9);
    rows.fill(vec![item("Episode", "only", None)]);
    assert_eq!(rows.selected, 0, "a stale index would index out of bounds");
}

#[test]
fn a_level_of_libraries_is_a_bare_grid_and_every_other_grid_sits_over_a_synopsis() {
    assert_eq!(mode(&[item("CollectionFolder", "lib", None)]), Mode::Grid);
    assert_eq!(mode(&[item("UserView", "collections", None)]), Mode::Grid);
    assert_eq!(mode(&[item("Episode", "e1", Some("s1"))]), Mode::Synopsis);
    assert_eq!(mode(&[item("Series", "s1", None)]), Mode::Synopsis);
    // A movie library grouped into collections lists both kinds on one screen.
    assert_eq!(mode(&[item("Movie", "m1", None)]), Mode::Synopsis);
    assert_eq!(mode(&[item("BoxSet", "b1", None)]), Mode::Synopsis);
    assert_eq!(mode(&[item("Video", "v1", None)]), Mode::List);
    assert_eq!(mode(&[]), Mode::List, "nothing to size the tiles from");
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
