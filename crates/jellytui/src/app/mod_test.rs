use super::*;
use crate::test_support::{app, item, playing, protocol};

fn playing_episode(item_id: &str) -> PlayerStatus {
    playing(item_id, "Paradise, Once More")
}

fn episode(id: &str) -> Item {
    item(serde_json::json!({
        "Id": id, "Name": id, "Type": "Episode", "IndexNumber": 1, "ParentIndexNumber": 1
    }))
}

fn selected(app: &App) -> usize {
    app.focused().map_or(0, |rows| rows.selected)
}

#[test]
fn leaving_the_search_screen_clears_the_query_so_it_does_not_reappear() {
    let mut app = app();
    app.apply(Intent::StartSearch);
    app.apply(Intent::Type('b'));
    app.apply(Intent::Type('e'));
    assert_eq!(app.query, "be");
    app.apply(Intent::Back);
    assert_eq!(app.screen, Screen::Home);
    assert!(app.query.is_empty());
}

#[tokio::test]
async fn search_results_are_loading_only_while_a_query_is_out() {
    let mut app = app();
    app.apply(Intent::StartSearch);
    assert!(!app.results.loading, "loading before anything was typed");

    app.apply(Intent::Type('b'));
    app.run_search();
    assert!(app.results.loading);

    app.apply(Intent::Backspace);
    app.run_search();
    assert!(
        !app.results.loading,
        "an empty query sends nothing to wait for"
    );
}

#[test]
fn up_and_down_move_between_the_home_shelves_and_each_keeps_its_cursor() {
    let mut app = app();
    app.shelf_mut(HomePane::Resume)
        .fill(vec![episode("a"), episode("b"), episode("c")]);
    app.shelf_mut(HomePane::NextUp).fill(vec![episode("z")]);
    app.apply(Intent::Bottom);
    assert_eq!(selected(&app), 2);

    // A shelf is a single row, so down leaves it rather than moving along it.
    app.apply(Intent::Down);
    assert_eq!(app.home_pane, HomePane::NextUp);
    assert_eq!(selected(&app), 0);

    app.apply(Intent::Up);
    assert_eq!(app.home_pane, HomePane::Resume);
    assert_eq!(selected(&app), 2, "the shelf forgot where it was left");
}

#[test]
fn a_shelf_that_arrives_shorter_than_the_cursor_pulls_it_back_into_range() {
    let mut app = app();
    app.shelf_mut(HomePane::Resume)
        .fill(vec![episode("a"), episode("b"), episode("c")]);
    app.apply(Intent::Bottom);
    app.on_msg(Msg::Home(HomePane::Resume, vec![episode("a")]));
    assert_eq!(selected(&app), 0);
}

/// Regression: only a keypress rescrolled, so a reload or a resize left the
/// cursor on a tile that was not drawn.
#[tokio::test(start_paused = true)]
async fn a_reload_keeps_the_cursor_on_a_tile_that_is_drawn() {
    let mut app = app();
    app.viewport = Size::new(120, 40);
    let tiles = || {
        (0..30)
            .map(|index| tile(&format!("t{index}")))
            .collect::<Vec<_>>()
    };
    app.shelf_mut(HomePane::Resume).fill(tiles());
    app.apply(Intent::Bottom);
    app.prepare_frame();

    app.on_msg(Msg::Home(HomePane::Resume, tiles()));
    app.prepare_frame();

    let shelf = app.shelf(HomePane::Resume);
    let cursor = app
        .covers
        .key(
            &shelf.items[shelf.selected],
            app.shelf_metrics(HomePane::Resume).unwrap().cover,
        )
        .unwrap();
    assert!(
        app.visible_covers().contains(&cursor),
        "the cursor's tile is scrolled off the shelf"
    );
}

#[tokio::test]
async fn playing_without_a_daemon_explains_itself_instead_of_doing_nothing() {
    let mut app = app();
    app.on_player(None);
    app.on_msg(Msg::Home(HomePane::Resume, vec![episode("e1")]));
    app.apply(Intent::Enter);
    assert!(
        app.message.contains("not connected"),
        "got {:?}",
        app.message
    );
}

#[tokio::test]
async fn a_command_before_the_first_poll_does_not_claim_the_daemon_is_absent() {
    let mut app = app();
    app.play(&episode("e1"));
    assert!(
        app.message.contains("checking for jellysink"),
        "got {:?}",
        app.message
    );
}

#[tokio::test]
async fn a_command_before_the_session_lookup_lands_says_so_rather_than_blaming_the_daemon() {
    let mut app = app();
    app.on_player(Some(playing_episode("e1")));
    app.play(&episode("e1"));
    assert!(
        app.message.contains("looking up the session"),
        "got {:?}",
        app.message
    );
}

#[tokio::test]
async fn an_item_lookup_that_lands_after_playback_moved_on_is_dropped() {
    // The reply describes the episode it was asked for, not the one playing
    // now, and the Playing screen must not caption the wrong thing.
    let mut app = app();
    app.on_player(Some(playing_episode("e1")));

    app.on_msg(Msg::PlayingItem {
        item_id: "e0".to_string(),
        item: Box::new(episode("e0")),
    });
    assert!(app.current_item().is_none());

    app.on_msg(Msg::PlayingItem {
        item_id: "e1".to_string(),
        item: Box::new(episode("e1")),
    });
    assert_eq!(app.current_item().map(|item| item.id.as_str()), Some("e1"));
}

#[test]
fn a_search_result_that_arrives_after_a_newer_query_is_discarded() {
    let mut app = app();
    app.search_generation = 2;
    app.on_msg(Msg::Search(1, vec![episode("stale")]));
    assert!(
        app.results.items.is_empty(),
        "an older response overwrote a newer one"
    );
    app.on_msg(Msg::Search(2, vec![episode("fresh")]));
    assert_eq!(app.results.items.len(), 1);
}

#[test]
fn rows_for_a_level_the_user_already_left_are_dropped() {
    let mut app = app();
    app.apply(Intent::Home);
    // Depth 3 does not exist; without the bounds check this indexes off the end.
    app.on_msg(Msg::Level(3, Source::Libraries, vec![episode("ghost")]));
    assert!(app.stack.is_empty());
}

#[tokio::test]
async fn rows_for_a_level_the_user_left_do_not_fill_the_one_that_replaced_it() {
    let mut app = app();
    app.screen = Screen::Browse;
    app.push("Movies", Source::Folder("movies".into()));
    app.apply(Intent::Back);
    app.push("Shows", Source::Folder("shows".into()));

    let movies = Source::Folder("movies".into());
    app.on_msg(Msg::Level(0, movies, vec![episode("m1")]));
    assert!(
        app.stack[0].rows.items.is_empty(),
        "the Movies reply landed on Shows"
    );
    assert!(app.stack[0].rows.loading);
}

#[test]
fn enter_on_an_empty_list_does_not_panic() {
    let mut app = app();
    app.apply(Intent::Enter);
    assert_eq!(app.screen, Screen::Home);
}

#[tokio::test]
async fn a_message_is_retired_by_the_next_keypress() {
    let mut app = app();
    app.on_msg(Msg::Home(HomePane::Resume, vec![episode("e1")]));
    app.apply(Intent::Enter);
    assert!(
        !app.message.is_empty(),
        "the complaint should be shown once"
    );
    app.apply(Intent::Down);
    assert!(
        app.message.is_empty(),
        "it must not sit in the header after the user has moved on"
    );
}

#[test]
fn arrows_in_a_list_do_not_double_as_back_and_open() {
    // Esc and Enter are the only way in and out; a list has no second axis for
    // left and right to move along.
    let row = |id: &str| item(serde_json::json!({"Id": id, "Name": id, "Type": "Video"}));
    let mut app = app();
    app.stack.push(Level::loading("Clips", Source::Libraries));
    app.stack
        .last_mut()
        .unwrap()
        .rows
        .fill(vec![row("a"), row("b")]);
    app.screen = Screen::Browse;

    app.apply(Intent::Left);
    assert_eq!(app.stack.len(), 1, "left must not pop the browse stack");
    app.apply(Intent::Right);
    assert_eq!(app.stack.len(), 1, "right must not open the row either");
    assert_eq!(selected(&app), 0);
}

fn tile(id: &str) -> Item {
    item(serde_json::json!({
        "Id": id, "Name": id, "Type": "Series", "ImageTags": { "Primary": "tag" }
    }))
}

/// A cursor that has been still asks for its covers at once: the throttle is
/// a rate limit, not a settling delay.
#[tokio::test(start_paused = true)]
async fn a_resting_cursor_fetches_its_covers_at_once() {
    let mut app = app();
    app.viewport = Size::new(120, 40);
    app.shelf_mut(HomePane::Resume)
        .fill(vec![tile("a"), tile("b"), tile("c")]);

    app.tick_covers();

    let wanted = app.visible_covers();
    assert!(!wanted.is_empty(), "the shelf has covers to ask for");
    assert!(
        app.cover_due.is_none(),
        "nothing was left waiting for a timer"
    );
    assert!(
        wanted.iter().all(|key| !app.covers.claim(key)),
        "the immediate batch claimed every visible cover"
    );
}

/// The reason the throttle exists: scrolling a library must not ask for a cover
/// per row the cursor passes through, only the last one it rests on.
#[tokio::test(start_paused = true)]
async fn a_moving_cursor_does_not_fetch_a_cover_per_row() {
    let mut app = app();
    app.viewport = Size::new(120, 40);
    app.shelf_mut(HomePane::Resume).fill(vec![tile("a")]);
    app.tick_covers();
    let ready_at = app.cover_ready_at.expect("the first batch opened a window");

    for row in 0..20 {
        app.shelf_mut(HomePane::Resume)
            .fill(vec![tile(&format!("row{row}"))]);
        app.tick_covers();
    }

    // Scheduled for when the window opens, not 120 ms after the last of the
    // twenty — a cursor coming to rest waits out what is left, not a fresh wait.
    assert_eq!(app.cover_due, Some(ready_at));
    assert_eq!(
        app.cover_ready_at,
        Some(ready_at),
        "no second batch went out inside the window"
    );
    let passed_through = app.visible_covers();
    assert!(!passed_through.is_empty());
    assert!(
        passed_through.iter().all(|key| app.covers.claim(key)),
        "nothing the cursor passed through was requested"
    );
}

/// A drag-resize is what evicts one: a key per intermediate size goes through
/// the cache while the screen itself does not move.
#[tokio::test(start_paused = true)]
async fn a_cover_evicted_while_the_cursor_stood_still_is_asked_for_again() {
    let mut app = app();
    app.viewport = Size::new(120, 40);
    app.shelf_mut(HomePane::Resume).fill(vec![tile("a")]);
    app.tick_covers();
    let key = app.visible_covers().pop().expect("the shelf wants a cover");
    app.covers.store(key.clone(), Some(protocol()));

    for index in 0..cover::CACHE_CAPACITY {
        let dragged = app
            .covers
            .key(&tile(&format!("drag{index}")), Size::new(4, 4))
            .unwrap();
        app.covers.store(dragged, Some(protocol()));
    }
    assert!(app.covers.protocol(&key).is_none(), "the cover was evicted");

    tokio::time::advance(COVER_THROTTLE * 2).await;
    app.tick_covers();

    assert!(
        !app.covers.claim(&key),
        "a blank tile the screen still wants was never asked for again"
    );
}

#[tokio::test(start_paused = true)]
async fn a_cover_whose_request_failed_is_not_asked_for_once_a_window_forever() {
    let mut app = app();
    app.viewport = Size::new(120, 40);
    app.shelf_mut(HomePane::Resume).fill(vec![tile("a")]);
    app.tick_covers();
    let key = app.visible_covers().pop().expect("the shelf wants a cover");
    let ready_at = app.cover_ready_at.expect("the first batch opened a window");

    app.on_msg(Msg::CoverFailed(key));
    tokio::time::advance(COVER_THROTTLE * 2).await;
    app.tick_covers();

    assert_eq!(
        app.cover_ready_at,
        Some(ready_at),
        "a second batch went out for a cover that had already failed"
    );
}

#[tokio::test]
async fn an_episode_ending_reloads_on_the_poll_after_the_one_that_saw_it() {
    let mut app = app();
    app.on_player(Some(playing_episode("e1")));

    app.on_player(Some(playing_episode("e2")));
    assert!(app.reload_due, "the transition to e2 did not arm a reload");

    app.on_player(Some(playing_episode("e2")));
    assert!(
        !app.reload_due,
        "the reload stayed armed and will fire again next poll"
    );
}

#[tokio::test]
async fn closing_the_mpv_window_arms_a_reload_like_an_episode_ending() {
    let mut app = app();
    app.on_player(Some(playing_episode("e1")));

    app.on_player(Some(PlayerStatus::idle("s".into(), "u".into())));
    assert!(app.reload_due);
}

#[tokio::test]
async fn the_first_poll_is_not_a_playback_change() {
    // Startup has just loaded Home; finding something already playing is not
    // news about it.
    let mut app = app();
    app.on_player(Some(playing_episode("e1")));
    assert!(!app.reload_due);
}

/// Regression: a lookup that failed, or had not answered yet, was sent again on
/// every poll — a warning and a header complaint a second.
#[tokio::test]
async fn the_playing_item_is_looked_up_once_per_episode_not_once_per_poll() {
    let mut app = app();
    let logs = crate::logs::capture(|| {
        app.on_player(Some(playing_episode("e1")));
        app.on_msg(Msg::Error("connection refused".into()));
        app.on_player(Some(playing_episode("e1")));
        app.on_player(Some(playing_episode("e1")));
    });
    let lookups = |logs: &LogBuffer| {
        let lines = logs.lines();
        lines
            .iter()
            .filter(|line| line.message.contains("now playing changed"))
            .count()
    };
    assert_eq!(lookups(&logs), 1);

    let logs = crate::logs::capture(|| app.on_player(Some(playing_episode("e2"))));
    assert_eq!(lookups(&logs), 1, "the next episode was not looked up");
}
