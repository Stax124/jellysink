use super::*;
use crate::cast::CastEvent;
use serde_json::json;

/// jellytui and the daemon are joined through Jellyfin rather than by a call,
/// so nothing but this keeps the command `play_now` sends in agreement with
/// the parser that has to recognise it.
#[test]
fn the_play_command_jellytui_sends_is_the_one_the_daemon_parses() {
    assert!(matches!(
        CastEvent::from_ws(
            "Play",
            &json!({ "PlayCommand": PLAY_NOW, "ItemIds": ["e1"] })
        ),
        Some(CastEvent::PlayNow { .. })
    ));
}
