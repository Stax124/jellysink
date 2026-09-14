use super::*;
use crate::test_support::app;

#[test]
fn the_update_key_does_nothing_but_say_so_when_there_is_no_offer() {
    let mut app = app();
    app.apply(Intent::Update);
    assert_eq!(app.quit, None);
    assert!(!app.message.is_empty());
}

#[test]
fn the_update_key_leaves_the_tui_so_the_install_gets_the_normal_screen() {
    let mut app = app();
    app.on_msg(Msg::UpdateAvailable("9.9.9".into()));
    app.apply(Intent::Update);
    assert_eq!(app.quit, Some(Exit::Update));
}

#[test]
fn the_offer_survives_the_keys_that_retire_a_message() {
    let mut app = app();
    app.on_msg(Msg::UpdateAvailable("9.9.9".into()));
    app.apply(Intent::Home);
    assert_eq!(app.update_offer.as_deref(), Some("9.9.9"));
}
