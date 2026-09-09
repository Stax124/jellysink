use super::super::nav::{Level, Source};
use super::*;
use crate::runtime::PlayerStatus;
use crate::runtime::status::NowPlaying;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use serde::Deserialize;

fn app() -> App {
    let credentials = crate::app::config::Credentials {
        server: "http://localhost:8096".into(),
        username: "test".into(),
        user_id: "u1".into(),
        access_token: "t1".into(),
        device_id: "d1".into(),
    };
    App::new(
        crate::jellyfin::auth::Api::from_credentials(&credentials).unwrap(),
        crate::app::config::Paths::from_override(Some(std::path::PathBuf::from("/nonexistent")))
            .unwrap(),
    )
}

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
    assert!(screen.contains("[3/103]"), "{screen}");
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
fn an_empty_library_says_so_rather_than_drawing_a_blank_box() {
    let mut app = app();
    let mut level = Level::loading("Movies", Source::Libraries);
    level.fill(Vec::new());
    app.stack.push(level);
    app.screen = Screen::Browse;
    assert!(drawn(&app).contains("nothing here"));
}

#[test]
fn a_watched_row_is_ticked_and_a_partly_watched_one_shows_its_percentage() {
    let mut app = app();
    let mut watched = episode();
    watched.user_data = Some(crate::jellyfin::model::UserData {
        played: true,
        playback_position_ticks: 0,
        played_percentage: None,
    });
    let mut partial = episode();
    partial.user_data = Some(crate::jellyfin::model::UserData {
        played: false,
        playback_position_ticks: 9_167_070_000,
        played_percentage: Some(64.0),
    });
    app.resume = vec![watched, partial];
    let screen = drawn(&app);
    assert!(screen.contains('✓'), "{screen}");
    assert!(screen.contains("64%"), "{screen}");
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
