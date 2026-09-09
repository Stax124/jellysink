use super::*;
use crate::app::config::{Credentials, Paths};
use crate::runtime::PlayerStatus;
use crate::runtime::status::NowPlaying;
use serde::Deserialize;

fn app() -> App {
    let credentials = Credentials {
        server: "http://localhost:8096".into(),
        username: "test".into(),
        user_id: "u1".into(),
        access_token: "t1".into(),
        device_id: "d1".into(),
    };
    App::new(
        Api::from_credentials(&credentials).unwrap(),
        Paths::from_override(Some(std::path::PathBuf::from("/nonexistent"))).unwrap(),
    )
}

fn playing_status() -> PlayerStatus {
    PlayerStatus {
        server: "s".into(),
        username: "u".into(),
        now_playing: Some(NowPlaying {
            item_id: "e1".into(),
            title: "Paradise, Once More".into(),
            position_ticks: 600_000_000,
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
fn switching_home_panes_resets_the_cursor_into_the_other_list() {
    let mut app = app();
    app.resume = vec![episode("a"), episode("b"), episode("c")];
    app.next_up = vec![episode("z")];
    app.apply(Intent::Bottom);
    assert_eq!(app.selected(), 2);
    app.apply(Intent::NextPane);
    // Keeping 2 here would select past the end of a one-row list.
    assert_eq!(app.home_pane, HomePane::NextUp);
    assert_eq!(app.selected(), 0);
}

#[tokio::test]
async fn transport_keys_without_a_daemon_explain_themselves_instead_of_doing_nothing() {
    let mut app = app();
    assert!(app.player.is_none());
    app.apply(Intent::PlayPause);
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
    app.apply(Intent::PlayPause);
    assert!(
        app.message.contains("still looking up"),
        "got {:?}",
        app.message
    );
}

#[tokio::test]
async fn seeking_while_nothing_plays_is_a_no_op() {
    let mut app = app();
    app.on_player(Some(PlayerStatus::idle("s".into(), "u".into())));
    app.apply(Intent::SeekBy(10));
    assert!(app.message.is_empty());
}

#[tokio::test]
async fn a_new_item_drops_the_previous_items_duration() {
    let mut app = app();
    app.on_player(Some(playing_status()));
    app.runtime_ticks = Some(("e1".to_string(), 14_220_809_999));
    assert_eq!(app.total_ticks(), Some(14_220_809_999));

    let mut next = playing_status();
    if let Some(now_playing) = next.now_playing.as_mut() {
        now_playing.item_id = "e2".into();
    }
    app.on_player(Some(next));
    // A stale total would mislabel the new episode until its own arrives.
    assert_eq!(app.total_ticks(), None);
}

#[test]
fn a_seek_is_relative_to_the_last_polled_position() {
    // The wire command is absolute, so this arithmetic is ours to get right.
    assert_eq!(seek_target(600_000_000, 10), 700_000_000);
    assert_eq!(seek_target(600_000_000, -10), 500_000_000);
}

#[test]
fn seeking_back_past_the_start_lands_on_zero_not_a_negative_position() {
    assert_eq!(seek_target(30_000_000, -10), 0);
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
async fn a_message_is_retired_by_the_next_keypress_so_the_key_hints_return() {
    let mut app = app();
    app.apply(Intent::PlayPause);
    assert!(
        !app.message.is_empty(),
        "the complaint should be shown once"
    );
    app.apply(Intent::Down);
    assert!(
        app.message.is_empty(),
        "it must not sit there hiding the hints"
    );
}
