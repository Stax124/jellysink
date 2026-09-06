use super::*;
#[test]
fn ticks_roundtrip() {
    assert!((ticks_to_seconds(150_000_000) - 15.0).abs() < f64::EPSILON);
    assert_eq!(seconds_to_ticks(15.0), 150_000_000);
}

#[test]

fn coalesce_uses_a_live_position() {
    assert_eq!(coalesce_position_ticks(Some(42.0), 0), 420_000_000);
}

#[test]

fn coalesce_keeps_last_ticks_when_live_is_missing() {
    assert_eq!(coalesce_position_ticks(None, 150_000_000), 150_000_000);
}

#[test]

fn coalesce_does_not_regress_to_zero_on_a_dead_sample() {
    // Closing mpv makes time-pos fail or return 0; a Stopped POST of 0
    // wipes the resume point Jellyfin already stored from progress ticks.
    assert_eq!(coalesce_position_ticks(Some(0.0), 150_000_000), 150_000_000);
    assert_eq!(
        coalesce_position_ticks(Some(f64::NAN), 150_000_000),
        150_000_000
    );
}

#[test]

fn coalesce_zero_when_nothing_has_played() {
    assert_eq!(coalesce_position_ticks(None, 0), 0);
    assert_eq!(coalesce_position_ticks(Some(0.0), 0), 0);
}

#[test]

fn coalesce_eof_reports_the_live_end() {
    assert_eq!(
        coalesce_position_ticks(Some(100.0), 900_000_000),
        1_000_000_000
    );
}
