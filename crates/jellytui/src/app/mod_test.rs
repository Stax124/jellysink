use super::*;
use crate::test_support::app;
use jellysink_core::status::NowPlaying;
use jellysink_core::status::PlayerStatus;
use serde::Deserialize;

fn playing_status() -> PlayerStatus {
    PlayerStatus {
        server: "s".into(),
        username: "u".into(),
        now_playing: Some(NowPlaying {
            item_id: "e1".into(),
            title: "Paradise, Once More".into(),
            position_ticks: 600_000_000,
            run_time_ticks: Some(14_220_809_999),
            is_paused: false,
            is_muted: false,
            volume: 50,
            has_next: true,
            has_previous: false,
            queue_index: 2,
            queue_len: 103,
            art_url: String::new(),
        }),
    }
}

fn episode(id: &str) -> Item {
    Item::deserialize(serde_json::json!({
        "Id": id, "Name": id, "Type": "Episode", "IndexNumber": 1, "ParentIndexNumber": 1
    }))
    .unwrap()
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

#[test]
fn up_and_down_move_between_the_home_shelves_and_each_keeps_its_cursor() {
    let mut app = app();
    app.resume
        .fill(vec![episode("a"), episode("b"), episode("c")]);
    app.next_up.fill(vec![episode("z")]);
    app.apply(Intent::Bottom);
    assert_eq!(app.selected(), 2);

    // A shelf is a single row, so down leaves it rather than moving along it.
    app.apply(Intent::Down);
    assert_eq!(app.home_pane, HomePane::NextUp);
    assert_eq!(app.selected(), 0);

    app.apply(Intent::Up);
    assert_eq!(app.home_pane, HomePane::Resume);
    assert_eq!(app.selected(), 2, "the shelf forgot where it was left");
}

#[test]
fn a_shelf_that_arrives_shorter_than_the_cursor_pulls_it_back_into_range() {
    let mut app = app();
    app.resume
        .fill(vec![episode("a"), episode("b"), episode("c")]);
    app.apply(Intent::Bottom);
    app.on_msg(Msg::Home(HomePane::Resume, vec![episode("a")]));
    assert_eq!(app.selected(), 0);
}

#[tokio::test]
async fn playing_without_a_daemon_explains_itself_instead_of_doing_nothing() {
    let mut app = app();
    assert!(app.player.is_none());
    app.on_msg(Msg::Home(HomePane::Resume, vec![episode("e1")]));
    app.apply(Intent::Enter);
    assert!(
        app.message.contains("not connected"),
        "got {:?}",
        app.message
    );
}

#[tokio::test]
async fn a_command_before_the_session_lookup_lands_says_so_rather_than_blaming_the_daemon() {
    let mut app = app();
    app.on_player(Some(playing_status()));
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
    app.on_player(Some(playing_status()));

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
    app.on_msg(Msg::Level(3, vec![episode("ghost")]));
    assert!(app.stack.is_empty());
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
    let mut app = app();
    app.stack.push(Level::loading("Movies", Source::Libraries));
    app.stack.last_mut().unwrap().fill(vec![episode("e1")]);
    app.screen = Screen::Browse;

    app.apply(Intent::Left);
    assert_eq!(app.stack.len(), 1, "left must not pop the browse stack");
    app.apply(Intent::Right);
    assert_eq!(app.stack.len(), 1, "right must not open the row either");
    assert_eq!(app.selected(), 0);
}

fn tile(id: &str) -> Item {
    Item::deserialize(serde_json::json!({
        "Id": id, "Name": id, "Type": "Series", "ImageTags": { "Primary": "tag" }
    }))
    .unwrap()
}

/// A cursor that has been still asks for its covers at once: the throttle is
/// a rate limit, not a settling delay.
#[tokio::test(start_paused = true)]
async fn a_resting_cursor_fetches_its_covers_at_once() {
    let mut app = app();
    app.viewport = Size::new(120, 40);
    app.resume.fill(vec![tile("a"), tile("b"), tile("c")]);

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
    app.resume.fill(vec![tile("a")]);
    app.tick_covers();
    let ready_at = app.cover_ready_at.expect("the first batch opened a window");

    for row in 0..20 {
        app.resume.fill(vec![tile(&format!("row{row}"))]);
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

fn protocol() -> Protocol {
    Picker::halfblocks()
        .new_protocol(
            image::DynamicImage::new_rgb8(4, 4),
            Size::new(2, 2),
            ratatui_image::Resize::Fit(None),
        )
        .unwrap()
}

/// A drag-resize is what evicts one: a key per intermediate size goes through
/// the cache while the screen itself does not move.
#[tokio::test(start_paused = true)]
async fn a_cover_evicted_while_the_cursor_stood_still_is_asked_for_again() {
    let mut app = app();
    app.viewport = Size::new(120, 40);
    app.resume.fill(vec![tile("a")]);
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
    app.resume.fill(vec![tile("a")]);
    app.tick_covers();
    let key = app.visible_covers().pop().expect("the shelf wants a cover");
    let ready_at = app.cover_ready_at.expect("the first batch opened a window");

    app.on_msg(Msg::CoverFailed {
        key,
        error: "connection refused".into(),
    });
    tokio::time::advance(COVER_THROTTLE * 2).await;
    app.tick_covers();

    assert_eq!(
        app.cover_ready_at,
        Some(ready_at),
        "a second batch went out for a cover that had already failed"
    );
}

fn playing_episode(item_id: &str) -> PlayerStatus {
    let mut status = playing_status();
    if let Some(now_playing) = status.now_playing.as_mut() {
        now_playing.item_id = item_id.into();
    }
    status
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
