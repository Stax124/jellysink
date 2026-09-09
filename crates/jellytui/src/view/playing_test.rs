use super::*;
use jellysink_core::status::NowPlaying;
use jellysink_core::status::PlayerStatus;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use serde::Deserialize;

const FONT_SIZE: FontSize = FontSize {
    width: 8,
    height: 16,
};

fn app() -> App {
    let credentials = jellysink_core::config::Credentials {
        server: "http://localhost:8096".into(),
        username: "test".into(),
        user_id: "u1".into(),
        access_token: "t1".into(),
        device_id: "d1".into(),
    };
    App::new(
        jellysink_core::jellyfin::auth::Api::from_credentials(&credentials).unwrap(),
        jellysink_core::config::Paths::from_override(Some(std::path::PathBuf::from(
            "/nonexistent",
        )))
        .unwrap(),
        ratatui_image::picker::Picker::halfblocks(),
        1.0,
    )
}

fn drawn(app: &App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(100, 26)).unwrap();
    terminal
        .draw(|frame| render(app, frame, frame.area()))
        .unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .chunks(100)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

fn playing(app: &mut App, item_id: &str, title: &str) {
    app.player_polled = true;
    app.player = Some(PlayerStatus {
        server: "s".into(),
        username: "u".into(),
        now_playing: Some(NowPlaying {
            item_id: item_id.into(),
            title: title.into(),
            position_ticks: 0,
            is_paused: false,
            is_muted: false,
            volume: 70,
            has_next: true,
            has_previous: false,
            queue_index: 0,
            queue_len: 9,
            art_url: String::new(),
        }),
    });
}

fn episode(id: &str, number: i64, name: &str) -> Item {
    Item::deserialize(serde_json::json!({
        "Id": id, "Name": name, "Type": "Episode", "SeriesName": "Severance",
        "IndexNumber": number, "ParentIndexNumber": 1, "ProductionYear": 2022,
        "OfficialRating": "TV-MA", "RunTimeTicks": 28_800_000_000i64,
        "Overview": "The severed floor keeps a wing nobody will admit exists."
    }))
    .unwrap()
}

#[test]
fn the_still_leaves_the_season_room_under_it() {
    let body = Rect::new(0, 0, 100, 26);
    let inner = block().inner(body);
    let still = still_rect(body, &episode("e5", 5, "Optics"), FONT_SIZE).unwrap();
    assert!(still.width > 1 && still.height > 1, "{still:?}");
    assert!(still.width <= STILL_WIDTH, "{still:?}");
    assert!(
        still.height * 2 <= inner.height,
        "{still:?} takes more than half of {inner:?}"
    );
}

#[test]
fn a_narrow_terminal_drops_the_still_rather_than_squeezing_the_synopsis() {
    let item = episode("e5", 5, "Optics");
    assert_eq!(
        still_size(Rect::new(0, 0, MIN_BANNER_WIDTH - 1, 26), &item, FONT_SIZE),
        None
    );
    assert!(still_size(Rect::new(0, 0, MIN_BANNER_WIDTH, 26), &item, FONT_SIZE).is_some());
}

#[test]
fn nothing_playing_says_so_rather_than_drawing_an_empty_frame() {
    let mut app = app();
    assert!(drawn(&app).contains("checking"));

    app.player_polled = true;
    assert!(drawn(&app).contains("jellysink not connected"));
}

#[test]
fn the_daemons_title_holds_the_screen_until_the_item_lookup_lands() {
    // The status socket has a title from the first poll; the synopsis and the
    // season need a round trip, and the screen must not be blank until then.
    let mut app = app();
    playing(&mut app, "e5", "Severance - s1e05 - Optics");
    assert!(drawn(&app).contains("Severance - s1e05 - Optics"));

    app.playing_item = Some(("e5".into(), episode("e5", 5, "Optics")));
    let screen = drawn(&app);
    assert!(screen.contains("S01E05  Optics"), "{screen}");
    assert!(
        screen.contains("Severance · 2022 · 48 min · TV-MA"),
        "{screen}"
    );
    assert!(screen.contains("severed floor"), "{screen}");
}

#[test]
fn an_item_left_over_from_the_previous_episode_is_not_described() {
    let mut app = app();
    playing(&mut app, "e6", "Severance - s1e06 - Hide and Seek");
    app.playing_item = Some(("e5".into(), episode("e5", 5, "Optics")));
    let screen = drawn(&app);
    assert!(!screen.contains("S01E05"), "{screen}");
    assert!(screen.contains("Hide and Seek"), "{screen}");
}

#[test]
fn the_rest_of_the_season_is_listed_under_the_synopsis() {
    let mut app = app();
    playing(&mut app, "e5", "Severance - s1e05 - Optics");
    app.playing_item = Some(("e5".into(), episode("e5", 5, "Optics")));
    app.playing_episodes.fill(vec![
        episode("e5", 5, "Optics"),
        episode("e6", 6, "Hide and Seek"),
    ]);
    let screen = drawn(&app);
    assert!(screen.contains("S01E06  Hide and Seek"), "{screen}");
}
