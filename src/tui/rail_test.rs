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

    let (list, rail) = split(Rect::new(0, 0, 120, 24));
    assert_eq!(rail.expect("a wide terminal has a rail").width, WIDTH);
    assert_eq!(list.width, 120 - WIDTH);
}

#[test]
fn an_episode_still_is_wide_and_a_series_poster_is_tall() {
    let rail = split(Rect::new(0, 0, 120, 26)).1.unwrap();
    let still = cover_rect(rail, &episode(), FONT_SIZE);
    let poster = cover_rect(rail, &series(), FONT_SIZE);

    // 16:9 is bounded by the rail's width; 2:3 has to give width up to stay
    // short enough to leave room for the text under it.
    assert!(still.width > poster.width, "{still:?} {poster:?}");
    assert!(poster.height > still.height, "{poster:?} {still:?}");

    let inner = block().inner(rail);
    for cover in [still, poster] {
        assert!(
            cover.width <= inner.width && cover.height <= inner.height,
            "{cover:?} escapes {inner:?}"
        );
    }
}

#[test]
fn the_meta_line_skips_whatever_the_server_did_not_send() {
    assert_eq!(meta(&episode()), "Severance · 2022 · 48 min · TV-MA");
    assert_eq!(meta(&series()), "");
}
