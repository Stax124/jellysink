use super::*;
use crate::nav::{Level, Source};
use crate::test_support::app;
use jellysink_core::jellyfin::model::Item;
use jellysink_core::status::NowPlaying;
use jellysink_core::status::PlayerStatus;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use serde::Deserialize;

fn drawn(app: &App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(90, 24)).unwrap();
    terminal.draw(|frame| render(app, frame)).unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .chunks(90)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

fn playing(title: &str, position_ticks: i64, is_paused: bool) -> PlayerStatus {
    PlayerStatus {
        server: "s".into(),
        username: "u".into(),
        now_playing: Some(NowPlaying {
            item_id: "e1".into(),
            title: title.to_string(),
            position_ticks,
            is_paused,
            is_muted: false,
            volume: 70,
            has_next: true,
            has_previous: true,
            queue_index: 2,
            queue_len: 103,
            art_url: String::new(),
        }),
    }
}

fn episode() -> Item {
    Item::deserialize(serde_json::json!({
        "Id": "e1", "Name": "Paradise, Once More", "Type": "Episode",
        "SeriesName": "Slime", "IndexNumber": 3, "ParentIndexNumber": 2,
        "RunTimeTicks": 14_220_809_999i64
    }))
    .unwrap()
}

#[test]
fn the_footer_names_the_paused_item_with_its_position_and_volume() {
    let mut app = app();
    // The title is the daemon's own `display_title`, not `Item::label` —
    // whoever started playback, the footer shows what mpv shows.
    app.player = Some(playing(
        "Slime - s2e03 - Paradise, Once More",
        9_167_070_000,
        true,
    ));
    app.player_polled = true;
    app.runtime_ticks = Some(("e1".to_string(), 14_220_809_999));
    let screen = drawn(&app);
    assert!(
        screen.contains("Slime - s2e03 - Paradise, Once More"),
        "{screen}"
    );
    assert!(screen.contains("15:17 / 23:42"), "{screen}");
    assert!(screen.contains("vol 70"), "{screen}");
    assert!(screen.contains("queue 3/103"), "{screen}");
    assert!(screen.contains('⏸'), "{screen}");
}

#[test]
fn the_footer_separates_not_yet_asked_from_asked_and_absent() {
    let mut app = app();
    let screen = drawn(&app);
    // Before the first poll answers there is nothing to report either way.
    assert!(screen.contains("checking"), "{screen}");
    assert!(!screen.contains("not connected"), "{screen}");

    app.player_polled = true;
    assert!(drawn(&app).contains("jellysink not connected"));

    app.player = Some(PlayerStatus::idle("s".into(), "u".into()));
    let screen = drawn(&app);
    assert!(screen.contains("nothing playing"), "{screen}");
    assert!(!screen.contains("not connected"), "{screen}");
}

#[test]
fn the_search_screen_advertises_the_quit_key_that_actually_works_there() {
    let mut app = app();
    app.screen = Screen::Search;
    let screen = drawn(&app);
    // `q` types into the query, so offering it as the quit key would strand
    // the user.
    assert!(screen.contains("^C quit"), "{screen}");
    assert!(!screen.contains("q quit"), "{screen}");
}

#[test]
fn the_daemon_dot_tells_the_three_states_apart() {
    // The glyph is the same in all three, so the colour carries the whole
    // signal — and "not asked yet" must not read as "gone".
    fn dot(app: &App) -> Option<Color> {
        daemon_status(app).spans.first()?.style.fg
    }

    let mut app = app();
    assert_eq!(dot(&app), Some(DIM));
    app.player_polled = true;
    assert_eq!(dot(&app), Some(BAD));
    app.player = Some(PlayerStatus::idle("s".into(), "u".into()));
    assert_eq!(dot(&app), Some(OK));
}

#[test]
fn a_complaint_goes_to_the_header_and_leaves_the_bindings_alone() {
    let mut app = app();
    app.message = "jellysink not connected".into();
    let screen = drawn(&app);
    assert!(screen.contains("jellysink not connected"), "{screen}");
    assert!(screen.contains("Enter play"), "{screen}");
}

#[test]
fn a_message_too_long_for_the_header_is_elided_rather_than_pushing_the_tabs_off() {
    let mut app = app();
    app.message = "x".repeat(200);
    let screen = drawn(&app);
    assert!(screen.contains(" / Search "), "{screen}");
    assert!(screen.contains('…'), "{screen}");
}

#[test]
fn a_caption_is_exactly_as_wide_as_the_box_it_is_drawn_into() {
    // A grid caption is a filled highlight bar and the header's message slot
    // is a fixed width, so a short string is padded and a long one elided.
    assert_eq!(to_width("Dune", 10), "Dune      ");
    assert_eq!(to_width("Blade Runner 2049", 10), "Blade Run…");
    assert_eq!(Line::from(to_width("Blade Runner 2049", 10)).width(), 10);
}

#[test]
fn the_rail_describes_the_row_under_the_cursor() {
    let mut app = app();
    let mut level = Level::loading("Season 2", Source::Libraries);
    let mut second = episode();
    second.id = "e2".into();
    second.name = Some("Rimuru's Rout".into());
    second.overview = Some("The federation musters at the western gate.".into());
    let mut first = episode();
    first.overview = Some("Nothing about this episode is on screen.".into());
    level.fill(vec![first, second]);
    level.selected = 1;
    app.stack.push(level);
    app.screen = Screen::Browse;

    let screen = drawn(&app);
    assert!(screen.contains("Details"), "{screen}");
    assert!(screen.contains("Rimuru's Rout"), "{screen}");
    assert!(screen.contains("federation musters"), "{screen}");
    assert!(!screen.contains("Nothing about this"), "{screen}");
}

#[test]
fn an_empty_library_says_so_rather_than_drawing_a_blank_box() {
    let mut app = app();
    let mut level = Level::loading("Movies", Source::Libraries);
    level.fill(Vec::new());
    app.stack.push(level);
    app.screen = Screen::Browse;
    assert!(drawn(&app).contains("nothing here"));
}

fn watched(mut item: Item, played: bool, position_ticks: i64) -> Item {
    item.user_data = Some(jellysink_core::jellyfin::model::UserData {
        played,
        playback_position_ticks: position_ticks,
        played_percentage: None,
        unplayed_item_count: None,
    });
    item
}

#[test]
fn a_watched_row_is_ticked_and_a_partly_watched_one_shows_its_percentage() {
    // Episode levels are lists, which have the width for a percentage.
    let mut app = app();
    let mut level = Level::loading("Season 2", Source::Libraries);
    level.fill(vec![
        watched(episode(), true, 0),
        watched(episode(), false, 9_167_070_000),
    ]);
    app.stack.push(level);
    app.screen = Screen::Browse;

    let screen = drawn(&app);
    assert!(screen.contains('✓'), "{screen}");
    assert!(screen.contains("64%"), "{screen}");
}

#[test]
fn a_tile_spends_its_one_caption_row_on_the_rating_not_on_a_tick() {
    // Home is a grid: progress is the bar under the cover, and a container
    // says what is left of it, so neither needs a tick to repeat.
    let mut app = app();
    let mut finished = watched(episode(), true, 0);
    finished.community_rating = Some(8.0);
    app.resume.fill(vec![finished]);

    let screen = drawn(&app);
    assert!(screen.contains("★ 8.0"), "{screen}");
    assert!(!screen.contains('✓'), "{screen}");
}

#[test]
fn home_shows_both_shelves_at_once_rather_than_one_behind_a_key() {
    let mut app = app();
    app.resume.fill(vec![episode()]);
    app.next_up.fill(vec![episode()]);
    let screen = drawn(&app);
    assert!(screen.contains("Continue Watching"), "{screen}");
    assert!(screen.contains("Next Up"), "{screen}");
}

#[test]
fn the_total_is_blank_until_the_duration_arrives_then_fills_in() {
    // The status socket carries no duration, so it is fetched separately and
    // lands a paint later.
    let mut app = app();
    app.player = Some(playing("Slime - s2e03", 9_167_070_000, false));
    app.player_polled = true;
    assert!(drawn(&app).contains("15:17 / 00:00"));

    app.runtime_ticks = Some(("e1".to_string(), 14_220_809_999));
    assert!(drawn(&app).contains("15:17 / 23:42"));
}
