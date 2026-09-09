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
fn a_caption_is_exactly_as_wide_as_the_cover_it_sits_under() {
    // The selected tile's caption is a filled highlight bar, so a short name
    // is padded and a long one elided rather than wrapped onto the next tile.
    assert_eq!(to_width("Dune", 10), "Dune      ");
    assert_eq!(to_width("Blade Runner 2049", 10), "Blade Run…");
    assert_eq!(Line::from(to_width("Blade Runner 2049", 10)).width(), 10);
}
