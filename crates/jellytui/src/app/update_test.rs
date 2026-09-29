use super::*;
use crate::test_support::app;

#[test]
fn the_update_key_does_nothing_but_say_so_when_there_is_no_offer() {
    let mut app = app();
    app.on_msg(Msg::UpdateChecked(UpdateCheck::Current));
    app.apply(Intent::Update);
    assert_eq!(app.quit, None);
    assert!(app.message.contains("up to date"), "got {:?}", app.message);
}

#[test]
fn the_update_key_does_not_claim_up_to_date_before_a_check_has_answered() {
    let mut app = app();
    app.apply(Intent::Update);
    assert!(
        app.message.contains("still checking"),
        "got {:?}",
        app.message
    );

    app.on_msg(Msg::UpdateChecked(UpdateCheck::Failed));
    app.apply(Intent::Update);
    assert!(app.message.contains("failed"), "got {:?}", app.message);
    assert_eq!(app.quit, None);
}

#[test]
fn the_update_key_leaves_the_tui_so_the_install_gets_the_normal_screen() {
    let mut app = app();
    app.on_msg(Msg::UpdateChecked(UpdateCheck::Available("9.9.9".into())));
    app.apply(Intent::Update);
    assert_eq!(app.quit, Some(Exit::Update));
}

#[test]
fn the_offer_survives_the_keys_that_retire_a_message() {
    let mut app = app();
    app.on_msg(Msg::UpdateChecked(UpdateCheck::Available("9.9.9".into())));
    app.apply(Intent::Home);
    assert_eq!(app.update, UpdateCheck::Available("9.9.9".into()));
}
