use super::*;
use serde_json::json;

#[test]
fn only_the_observed_track_properties_become_events() {
    assert!(matches!(
        mpv_event_for(&IpcMessage::PropertyChange {
            property: SUBTITLE_TRACK_PROPERTY.into()
        }),
        Some(MpvEvent::SubtitleTrackChanged)
    ));
    assert!(matches!(
        mpv_event_for(&IpcMessage::PropertyChange {
            property: AUDIO_TRACK_PROPERTY.into()
        }),
        Some(MpvEvent::AudioTrackChanged)
    ));
    // Polled every second rather than observed; a change event for it would
    // be a property we never asked about.
    assert!(
        mpv_event_for(&IpcMessage::PropertyChange {
            property: "volume".into()
        })
        .is_none()
    );
}

/// The whole point of [`SelectedTrack`]: `no` and `auto` are different
/// answers, and reading `auto` as "off" would record a file that is still
/// loading as the user switching subtitles off.

#[test]
fn a_track_property_tells_off_apart_from_not_yet_decided() {
    assert_eq!(
        selected_track_from_property(&json!(3)),
        SelectedTrack::Id(3)
    );
    assert_eq!(
        selected_track_from_property(&json!(false)),
        SelectedTrack::Off
    );
    assert_eq!(
        selected_track_from_property(&json!("no")),
        SelectedTrack::Off
    );
    assert_eq!(
        selected_track_from_property(&json!("auto")),
        SelectedTrack::Unresolved
    );
    assert_eq!(
        selected_track_from_property(&Value::Null),
        SelectedTrack::Unresolved
    );
}

#[test]
fn end_file_reasons_parse_to_their_variants() {
    assert_eq!(EndFileReason::parse(Some("eof")), EndFileReason::Eof);
    assert_eq!(
        EndFileReason::parse(Some("redirect")),
        EndFileReason::Redirect
    );
    assert_eq!(EndFileReason::parse(Some("stop")), EndFileReason::Stop);
    assert_eq!(EndFileReason::parse(Some("quit")), EndFileReason::Quit);
    assert_eq!(EndFileReason::parse(Some("error")), EndFileReason::Error);
}

#[test]
fn an_unknown_or_missing_reason_becomes_other() {
    assert_eq!(EndFileReason::parse(None), EndFileReason::Other);
    assert_eq!(
        EndFileReason::parse(Some("something-new")),
        EndFileReason::Other
    );
}

#[test]
fn display_round_trips_mpv_spelling() {
    for name in ["eof", "redirect", "stop", "quit", "error"] {
        assert_eq!(EndFileReason::parse(Some(name)).to_string(), name);
    }
    assert_eq!(EndFileReason::Other.to_string(), "unknown");
}
