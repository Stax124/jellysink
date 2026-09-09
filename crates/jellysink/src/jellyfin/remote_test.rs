use super::*;
use crate::cast::CastEvent;

/// What the server delivers over the WebSocket for a `Playstate` POST.
fn delivered(command: PlaystateCommand, seek_ticks: Option<i64>) -> CastEvent {
    let mut data = json!({ "Command": command.as_str() });
    if let Some(seek_ticks) = seek_ticks {
        data["SeekPositionTicks"] = json!(seek_ticks);
    }
    CastEvent::from_ws("Playstate", &data).expect("jellysink should parse its own command")
}

/// The two halves are wired through Jellyfin, so nothing but a test keeps the
/// spellings here and in `cast.rs` in agreement.
#[test]
fn every_playstate_command_round_trips_into_the_event_it_names() {
    assert_eq!(
        delivered(PlaystateCommand::PlayPause, None),
        CastEvent::PlayPause
    );
    assert_eq!(delivered(PlaystateCommand::Stop, None), CastEvent::Stop);
    assert_eq!(
        delivered(PlaystateCommand::NextTrack, None),
        CastEvent::Next
    );
    assert_eq!(
        delivered(PlaystateCommand::PreviousTrack, None),
        CastEvent::Previous
    );
    assert_eq!(
        delivered(PlaystateCommand::Seek, Some(9_167_070_000)),
        CastEvent::Seek {
            ticks: 9_167_070_000
        }
    );
}

#[test]
fn the_general_commands_the_footer_sends_round_trip_too() {
    let general = |name: &str, arguments: Value| {
        CastEvent::from_ws(
            "GeneralCommand",
            &json!({"Name": name, "Arguments": arguments}),
        )
    };
    // Volume goes over the wire as a string, which is what the web app sends
    // and what `value_as_i64` has to cope with.
    assert_eq!(
        general("SetVolume", json!({"Volume": "45"})),
        Some(CastEvent::SetVolume { volume: 45 })
    );
    assert_eq!(
        general("ToggleMute", json!({})),
        Some(CastEvent::ToggleMute)
    );
    assert_eq!(
        general("ToggleFullscreen", json!({})),
        Some(CastEvent::ToggleFullscreen)
    );
}
