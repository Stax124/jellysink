use super::*;
use serde::Deserialize;

const FONT_SIZE: FontSize = FontSize {
    width: 8,
    height: 16,
};

fn episode() -> Item {
    Item::deserialize(serde_json::json!({
        "Id": "e1", "Name": "The Grim Barbarity of Optics", "Type": "Episode",
        "SeriesName": "Severance", "IndexNumber": 5, "ParentIndexNumber": 1,
        "ProductionYear": 2022, "OfficialRating": "TV-MA",
        "RunTimeTicks": 28_800_000_000i64
    }))
    .unwrap()
}

fn series() -> Item {
    Item::deserialize(serde_json::json!({
        "Id": "s1", "Name": "Severance", "Type": "Series"
    }))
    .unwrap()
}

#[test]
fn a_narrow_terminal_keeps_the_whole_body_for_the_list() {
    let (list, rail) = split(Rect::new(0, 0, 80, 24));
    assert_eq!(list.width, 80);
    assert!(rail.is_none());
}

#[test]
fn the_rail_takes_half_the_body_and_grows_with_it() {
    for body_width in [90, 120, 161, 220, 400] {
        let (list, rail) = split(Rect::new(0, 0, body_width, 24));
        let rail = rail.expect("a wide terminal has a rail").width;
        assert_eq!(list.width + rail, body_width, "at {body_width}");
        assert!(
            list.width.abs_diff(rail) <= 1,
            "{list:?} and {rail} at {body_width}"
        );
    }
}

#[test]
fn a_still_is_wider_than_a_poster_and_neither_crowds_out_the_text() {
    let rail = split(Rect::new(0, 0, 120, 26)).1.unwrap();
    let still = cover_rect(rail, &episode(), FONT_SIZE);
    let poster = cover_rect(rail, &series(), FONT_SIZE);
    assert!(still.width > poster.width, "{still:?} {poster:?}");

    // Half a body is wide enough that the height cap binds both shapes, and
    // it is what leaves the synopsis somewhere to go.
    let inner = block().inner(rail);
    for cover in [still, poster] {
        assert!(
            cover.width <= inner.width && cover.height <= inner.height,
            "{cover:?} escapes {inner:?}"
        );
        assert!(
            cover.height * 5 <= inner.height * 3,
            "{cover:?} takes the rail the text needs"
        );
    }
}

#[test]
fn the_meta_line_skips_whatever_the_server_did_not_send() {
    assert_eq!(meta(&episode()), "Severance · 2022 · 48 min · TV-MA");
    assert_eq!(meta(&series()), "");
}
