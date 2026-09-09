use super::*;

const FONT_SIZE: FontSize = FontSize {
    width: 8,
    height: 16,
};

fn columns(metrics: &Metrics) -> u16 {
    u16::try_from(metrics.columns).unwrap()
}

#[test]
fn tiles_fill_the_row_and_posters_pack_tighter_than_stills() {
    let area = Rect::new(0, 0, 94, 24);
    let posters = metrics(area, 2.0 / 3.0, FONT_SIZE);
    let stills = metrics(area, 16.0 / 9.0, FONT_SIZE);

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
    let grid = metrics(Rect::new(0, 0, 94, 40), 2.0 / 3.0, FONT_SIZE);
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
    let short = metrics(Rect::new(0, 0, 158, 20), 2.0 / 3.0, FONT_SIZE);
    let tall = metrics(Rect::new(0, 0, 158, 54), 2.0 / 3.0, FONT_SIZE);

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
    let grid = metrics(Rect::new(0, 0, 88, 18), aspect, FONT_SIZE);

    assert_eq!(grid.rows, 1, "{grid:?}");
    assert_eq!(
        grid.cover.height,
        cover::rows_for(grid.cover.width, aspect, FONT_SIZE),
        "{grid:?} is capped below its own aspect"
    );
}

#[test]
fn a_wide_area_keeps_a_row_of_stills_rather_than_a_couple_of_huge_ones() {
    let grid = metrics(Rect::new(0, 0, 238, 54), 16.0 / 9.0, FONT_SIZE);
    assert!(grid.columns >= usize::from(MIN_COLUMNS), "{grid:?}");
}
