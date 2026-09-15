use super::*;
use crate::cover::CoverDisk;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui_image::picker::Picker;
use serde::Deserialize;
use serde_json::json;

const FONT_SIZE: FontSize = FontSize {
    width: 8,
    height: 16,
};

fn columns(metrics: &Metrics) -> u16 {
    u16::try_from(metrics.columns).unwrap()
}

/// More items than any of these areas can hold, for the cases that are about
/// the geometry rather than about a level too short to fill it.
fn full(target_rows: u16) -> Shape {
    Shape {
        target_rows,
        item_count: 200,
    }
}

#[test]
fn tiles_fill_the_row_and_posters_pack_tighter_than_stills() {
    let area = Rect::new(0, 0, 94, 24);
    let posters = metrics(area, 2.0 / 3.0, FONT_SIZE, full(TARGET_ROWS));
    let stills = metrics(area, 16.0 / 9.0, FONT_SIZE, full(TARGET_ROWS));

    assert!(posters.columns > stills.columns, "{posters:?} {stills:?}");
    assert!(stills.tile.width > posters.tile.width, "{stills:?}");

    for grid in [posters, stills] {
        let used = grid.tile.width * columns(&grid) + GAP * (columns(&grid) - 1);
        assert!(used <= area.width, "{grid:?} overruns {}", area.width);
        assert!(
            area.width - used < grid.tile.width,
            "{grid:?} leaves room for another tile"
        );
    }
}

#[test]
fn a_tile_reserves_the_rows_under_its_cover_for_the_caption() {
    let grid = metrics(
        Rect::new(0, 0, 94, 40),
        2.0 / 3.0,
        FONT_SIZE,
        full(TARGET_ROWS),
    );
    assert_eq!(grid.tile.height, grid.cover.height + LABEL_HEIGHT);
    assert!(grid.rows >= 2, "{grid:?}");
}

#[test]
fn scrolling_moves_by_the_least_that_brings_the_cursor_back_on_screen() {
    let grid = Metrics {
        columns: 5,
        rows: 3,
        tile: Size::new(18, 15),
        cover: Size::new(16, 12),
    };
    assert_eq!(
        scroll_to(0, 7, &grid),
        0,
        "an on-screen row does not scroll"
    );
    assert_eq!(
        scroll_to(0, 15, &grid),
        1,
        "one row past the bottom, not a page"
    );
    assert_eq!(
        scroll_to(4, 12, &grid),
        2,
        "moving up lands on the row itself"
    );
}

#[test]
fn a_tall_area_spends_its_height_on_bigger_tiles_rather_than_more_rows() {
    let short = metrics(
        Rect::new(0, 0, 158, 20),
        2.0 / 3.0,
        FONT_SIZE,
        full(TARGET_ROWS),
    );
    let tall = metrics(
        Rect::new(0, 0, 158, 54),
        2.0 / 3.0,
        FONT_SIZE,
        full(TARGET_ROWS),
    );

    assert_eq!(tall.rows, usize::from(TARGET_ROWS), "{tall:?}");
    assert!(
        tall.cover.width > short.cover.width,
        "{tall:?} vs {short:?}"
    );
    assert!(tall.columns < short.columns, "{tall:?} vs {short:?}");
}

#[test]
fn a_short_area_keeps_its_tiles_rather_than_shrinking_them_for_a_second_row() {
    // Two rows are out of reach at eighteen rows high, and capping the cover
    // to chase them would only make the one row that does fit smaller.
    let aspect = 2.0 / 3.0;
    let grid = metrics(
        Rect::new(0, 0, 88, 18),
        aspect,
        FONT_SIZE,
        full(TARGET_ROWS),
    );

    assert_eq!(grid.rows, 1, "{grid:?}");
    assert_eq!(
        grid.cover.height,
        cover::rows_for(grid.cover.width, aspect, FONT_SIZE),
        "{grid:?} is capped below its own aspect"
    );
}

#[test]
fn a_level_too_short_to_fill_the_grid_reserves_no_row_for_what_it_has_not_got() {
    // Three libraries on a wide screen: the tiles spread over their own count
    // rather than a quarter each, and no height is held back for a second row.
    let area = Rect::new(0, 0, 158, 39);
    let shape = Shape {
        target_rows: TARGET_ROWS,
        item_count: 3,
    };
    let short = metrics(area, 16.0 / 9.0, FONT_SIZE, shape);
    let packed = metrics(area, 16.0 / 9.0, FONT_SIZE, full(TARGET_ROWS));

    assert_eq!(short.rows, 1, "{short:?}");
    assert_eq!(short.columns, 3, "{short:?}");
    assert!(
        short.cover.width > packed.cover.width,
        "{short:?} vs {packed:?}"
    );
}

#[test]
fn tiles_hang_from_the_top_rather_than_floating_in_the_middle_of_the_body() {
    // Centring a block against rows the level never draws is what leaves half
    // a tile of blank above the only row there is.
    let covers = Covers::new(Picker::halfblocks(), CoverDisk::disabled());
    let libraries: Vec<Item> = (0..3)
        .map(|index| {
            item(json!({"Id": index.to_string(), "Name": "Lib", "Type": "CollectionFolder"}))
        })
        .collect();
    let area = Rect::new(0, 0, 60, 30);
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| {
            render(
                frame,
                area,
                Line::from(" Libraries "),
                View {
                    items: &libraries,
                    selected: 0,
                    offset: 0,
                    rows: TARGET_ROWS,
                    focused: true,
                },
                &covers,
            );
        })
        .unwrap();

    let buffer = terminal.backend().buffer();
    let rule = (0..area.height)
        .find(|y| (0..area.width).any(|x| buffer[(x, *y)].symbol() == TRACK))
        .expect("the rule under a cover is drawn");
    let grid = metrics(
        inner(area),
        cover::primary_aspect(&libraries[0]),
        covers.font_size(),
        Shape {
            target_rows: TARGET_ROWS,
            item_count: libraries.len(),
        },
    );
    assert_eq!(rule, inner(area).y + grid.cover.height);
}

#[test]
fn a_wide_area_keeps_a_row_of_stills_rather_than_a_couple_of_huge_ones() {
    let grid = metrics(
        Rect::new(0, 0, 238, 54),
        16.0 / 9.0,
        FONT_SIZE,
        full(TARGET_ROWS),
    );
    assert!(grid.columns >= usize::from(MIN_COLUMNS), "{grid:?}");
}

#[test]
fn a_shelf_fits_a_row_of_tiles_into_half_a_body_however_short_it_is() {
    // `render` skips a tile taller than its area, so without the one-row cap
    // a shelf on a small terminal draws nothing at all.
    for height in [8, 10, 16, 23] {
        for aspect in [2.0 / 3.0, 16.0 / 9.0] {
            let area = Rect::new(0, 0, 88, height);
            let shelf = metrics(area, aspect, FONT_SIZE, full(SHELF_ROWS));
            assert_eq!(shelf.rows, 1, "{shelf:?} in {area:?}");
            assert!(shelf.tile.height <= height, "{shelf:?} overruns {area:?}");
            assert!(shelf.columns > 1, "{shelf:?} in {area:?}");
        }
    }
}

#[test]
fn a_shelf_spends_the_width_the_cover_cannot_use_on_more_tiles() {
    let area = Rect::new(0, 0, 88, 10);
    let shelf = metrics(area, 2.0 / 3.0, FONT_SIZE, full(SHELF_ROWS));
    assert!(
        shelf.tile.width < minimum_tile_width(2.0 / 3.0),
        "{shelf:?}"
    );
    assert_eq!(shelf.cover.width, shelf.tile.width - 2, "{shelf:?}");
}

fn item(raw: serde_json::Value) -> Item {
    Item::deserialize(raw).unwrap()
}

#[test]
fn a_caption_carries_what_is_left_and_how_it_rates_and_nothing_it_cannot_fit() {
    let part_watched = item(json!({
        "Id": "s1", "Name": "Slime", "Type": "Series", "ProductionYear": 2018,
        "CommunityRating": 8.0, "RunTimeTicks": 14400000000i64, "OfficialRating": "TV-14",
        "UserData": {"PlayedPercentage": 55.3, "UnplayedItemCount": 46, "Played": false}
    }));
    assert_eq!(caption_meta(&part_watched), "46 left · ★ 8.0");

    let finished = item(json!({
        "Id": "s2", "Name": "Alya", "Type": "Series", "ProductionYear": 2024,
        "CommunityRating": 7.5, "OfficialRating": "TV-14",
        "UserData": {"UnplayedItemCount": 0, "Played": true}
    }));
    assert_eq!(caption_meta(&finished), "★ 7.5");

    let episode = item(json!({
        "Id": "e1", "Name": "Falmuth", "Type": "Episode", "IndexNumber": 4,
        "CommunityRating": 7.9, "RunTimeTicks": 14220350000i64
    }));
    assert_eq!(caption_meta(&episode), "★ 7.9");

    let unrated = item(json!({"Id": "u1", "Name": "Home Video", "Type": "Video"}));
    assert_eq!(caption_meta(&unrated), "");
}

#[test]
fn a_part_watched_series_fills_its_rule_rather_than_leaving_it_empty() {
    let series = item(json!({
        "Id": "s1", "Name": "Slime", "Type": "Series",
        "RunTimeTicks": 14400000000i64,
        "UserData": {"PlayedPercentage": 55.3, "PlaybackPositionTicks": 0, "Played": false}
    }));
    let filled = |line: Line<'static>| line.spans[0].content.chars().count();
    assert_eq!(filled(watched_rule(&series, 20)), 11);

    let untouched = item(json!({"Id": "s2", "Name": "Bebop", "Type": "Series"}));
    assert_eq!(filled(watched_rule(&untouched, 20)), 0);
}
