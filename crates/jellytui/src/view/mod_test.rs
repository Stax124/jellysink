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
    drawn_at(app, 90)
}

fn drawn_at(app: &App, width: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, 24)).unwrap();
    terminal.draw(|frame| render(app, frame)).unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .chunks(usize::from(width))
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The bindings row is the last one drawn.
fn hints_at(app: &App, width: u16) -> String {
    drawn_at(app, width)
        .lines()
        .next_back()
        .expect("a drawn screen has rows")
        .trim_end()
        .to_string()
}

fn playing(title: &str, position_ticks: i64, is_paused: bool) -> PlayerStatus {
    PlayerStatus {
        server: "s".into(),
        username: "u".into(),
        now_playing: Some(NowPlaying {
            item_id: "e1".into(),
            title: title.to_string(),
            position_ticks,
            run_time_ticks: Some(14_220_809_999),
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
fn an_offered_update_is_named_in_the_header_with_the_key_that_takes_it() {
    let mut app = app();
    assert!(!drawn(&app).contains("9.9.9"));
    app.update_offer = Some("9.9.9".into());
    let screen = drawn(&app);
    assert!(screen.contains("↑9.9.9 u"), "{screen}");
}

#[test]
fn an_offered_update_elides_the_message_rather_than_the_tabs() {
    let mut app = app();
    app.update_offer = Some("9.9.9".into());
    app.message = "x".repeat(200);
    let screen = drawn_at(&app, 80);
    assert!(screen.contains(" / Search "), "{screen}");
    assert!(screen.contains("↑9.9.9 u"), "{screen}");
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
fn the_libraries_screen_is_a_wall_of_tiles_rather_than_a_list_and_a_rail() {
    let mut app = app();
    let mut level = Level::loading("Libraries", Source::Libraries);
    level.fill(vec![
        Item::deserialize(serde_json::json!({
            "Id": "l1", "Name": "Movies", "Type": "CollectionFolder",
            "PrimaryImageAspectRatio": 1.777_777_777_777_777_7
        }))
        .unwrap(),
    ]);
    app.stack.push(level);
    app.screen = Screen::Browse;

    let screen = drawn(&app);
    assert!(screen.contains("Movies"), "{screen}");
    assert!(
        !screen.contains("Details"),
        "a grid has no rail beside it\n{screen}"
    );
    assert!(screen.contains("Enter open"), "{screen}");
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
fn an_item_the_server_gives_no_duration_for_draws_a_blank_total() {
    let mut app = app();
    let mut status = playing("Slime - s2e03", 9_167_070_000, false);
    if let Some(now_playing) = status.now_playing.as_mut() {
        now_playing.run_time_ticks = None;
    }
    app.player = Some(status);
    app.player_polled = true;
    assert!(drawn(&app).contains("15:17 / 00:00"));
}

#[test]
fn the_log_pane_names_itself_in_the_header_only_while_it_is_up() {
    let mut app = app();
    assert!(!drawn(&app).contains("L Logs"));

    app.screen = Screen::Logs;
    let screen = drawn(&app);
    assert!(screen.contains("L Logs"));
    assert!(
        screen.contains("RUST_LOG=jellytui=debug"),
        "an empty buffer has to say why it is empty, not just draw nothing"
    );
    assert!(screen.contains("c clear"), "the hint row is the pane's own");
}

#[test]
fn a_captured_event_is_drawn_with_its_level_and_target() {
    let logs = crate::logs::LogBuffer::new();
    logs.push_line("played item=The Bear".into());
    let mut app = crate::test_support::app_with_logs(logs);
    app.screen = Screen::Logs;

    let screen = drawn(&app);
    assert!(screen.contains("INFO"));
    assert!(screen.contains("played item=The Bear"));
}

#[test]
fn the_hint_row_offers_reload_and_not_the_keys_mpv_already_owns() {
    let mut app = app();
    for screen in [Screen::Home, Screen::Browse, Screen::Playing] {
        app.screen = screen;
        let hints = hints_at(&app, 90);
        assert!(hints.contains("r reload"), "{screen:?}: {hints}");
        for gone in ["pause", "seek", "vol", "mute", "full", "track"] {
            assert!(
                !hints.contains(gone),
                "{screen:?} still offers {gone}: {hints}"
            );
        }
    }
}

#[test]
fn every_hint_row_fits_an_eighty_column_terminal() {
    // A row wider than the terminal is cut mid-word with no ellipsis, so the
    // bindings nearest the end simply stop existing for the user.
    let mut app = app();
    for screen in [
        Screen::Home,
        Screen::Browse,
        Screen::Search,
        Screen::Playing,
        Screen::Logs,
    ] {
        app.screen = screen;
        let hints = hints_at(&app, 80);
        assert!(
            hints.chars().count() < 80,
            "{screen:?} fills or overruns the row: {hints}"
        );
    }
}
