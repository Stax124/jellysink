use super::*;

/// The fields that carry identity; the rest come from `Default`.
fn track(index: i64, language: &str, title: &str) -> TrackId {
    TrackId {
        index,
        language: Some(language.to_string()),
        title: Some(title.to_string()),
        ..Default::default()
    }
}

/// The case this whole module exists for: one language, two tracks, and the
/// indexes swapped between episodes.
fn signs_and_dialogue(signs: i64, dialogue: i64) -> Vec<TrackId> {
    vec![
        track(signs, "eng", "Signs and Songs"),
        track(dialogue, "eng", "Dialogue"),
    ]
}

#[test]
fn the_right_language_outranks_the_right_track_name_in_the_wrong_language() {
    let wanted = track(2, "eng", "Signs");
    let candidates = vec![track(2, "spa", "Signs"), track(3, "eng", "Dialogue")];
    assert_eq!(best_match(&wanted, &candidates).map(|c| c.index), Some(3));
}

#[test]
fn the_display_title_matches_when_the_provider_supplies_no_track_title() {
    let named_only_by_display = |index: i64, display: &str| TrackId {
        index,
        language: Some("eng".into()),
        display_title: Some(display.into()),
        ..Default::default()
    };
    let wanted = named_only_by_display(1, "English - Dialogue - SRT");
    let candidates = vec![
        named_only_by_display(4, "English - Signs and Songs - ASS"),
        named_only_by_display(5, "English - Dialogue - SRT"),
    ];
    assert_eq!(best_match(&wanted, &candidates).map(|c| c.index), Some(5));
}

#[test]
fn the_forced_flag_breaks_a_tie_between_two_tracks_of_the_same_language() {
    // No track names at all — a common case for raw remuxes, where the
    // forced flag is the only thing separating signs from dialogue.
    let unnamed = |index: i64, is_forced: bool| TrackId {
        index,
        language: Some("eng".into()),
        is_forced,
        ..Default::default()
    };
    let wanted = unnamed(2, false);
    let candidates = vec![unnamed(4, true), unnamed(5, false)];
    assert_eq!(best_match(&wanted, &candidates).map(|c| c.index), Some(5));
}

#[test]
fn a_track_renamed_by_another_provider_still_matches_on_language() {
    let wanted = track(2, "eng", "Dialogue");
    let candidates = vec![track(7, "jpn", "Full"), track(8, "eng", "Full Subtitles")];
    assert_eq!(best_match(&wanted, &candidates).map(|c| c.index), Some(8));
}

#[test]
fn matching_ignores_case_and_surrounding_whitespace() {
    let wanted = track(1, "ENG", "  Dialogue ");
    let candidates = vec![track(4, "eng", "dialogue")];
    assert_eq!(best_match(&wanted, &candidates).map(|c| c.index), Some(4));
}

#[test]
fn an_empty_or_undefined_language_is_not_a_language_match() {
    // Jellyfin sends "" and "und" rather than omitting the field. Treating
    // either as a value would match every unlabelled track to every other.
    for language in ["", "und", "  "] {
        let wanted = TrackId {
            index: 1,
            language: Some(language.into()),
            ..Default::default()
        };
        let candidates = vec![TrackId {
            index: 2,
            language: Some(language.into()),
            ..Default::default()
        }];
        assert!(
            best_match(&wanted, &candidates).is_none(),
            "{language:?} should carry no identity"
        );
    }
}

#[test]
fn flags_only_agreement_never_makes_an_unrelated_track_eligible() {
    let wanted = track(1, "eng", "Dialogue");
    // Same flags, same codec, same index — and nothing in common that means
    // anything.
    let candidates = vec![track(1, "jpn", "Signs")];
    assert!(best_match(&wanted, &candidates).is_none());
}

#[test]
fn a_stream_with_no_language_or_name_cannot_be_remembered() {
    let candidates = vec![TrackId {
        index: 3,
        ..Default::default()
    }];
    assert_eq!(TrackPreference::from_selection(&candidates, 3), None);
}

#[test]
fn an_index_the_item_cannot_select_cannot_be_remembered() {
    let candidates = signs_and_dialogue(2, 3);
    assert_eq!(TrackPreference::from_selection(&candidates, 9), None);
}

#[test]
fn a_selectable_named_stream_is_remembered_whole() {
    let candidates = signs_and_dialogue(2, 3);
    assert_eq!(
        TrackPreference::from_selection(&candidates, 3),
        Some(TrackPreference::Stream(track(3, "eng", "Dialogue")))
    );
}

#[test]
fn off_is_a_decision_even_when_the_item_has_no_such_streams_at_all() {
    assert_eq!(
        TrackPreference::from_selection(&[], -1),
        Some(TrackPreference::Off)
    );
}

#[test]
fn the_lowest_index_wins_between_two_indistinguishable_tracks() {
    let wanted = track(9, "eng", "Dialogue");
    let candidates = vec![track(5, "eng", "Dialogue"), track(4, "eng", "Dialogue")];
    assert_eq!(best_match(&wanted, &candidates).map(|c| c.index), Some(4));
}

#[test]
fn the_shared_memory_round_trips_and_can_be_forgotten() {
    let memory = TrackMemory::default();
    assert_eq!(remembered_track(&memory), None);
    remember_track(&memory, Some(TrackPreference::Off));
    assert_eq!(remembered_track(&memory), Some(TrackPreference::Off));
    // A second session holding the same slot sees the choice; this is what
    // survives a websocket reconnect.
    assert_eq!(
        remembered_track(&TrackMemory::clone(&memory)),
        Some(TrackPreference::Off)
    );
    remember_track(&memory, None);
    assert_eq!(remembered_track(&memory), None);
}

/// The two slots are the same type; only the fields that hold them keep
/// them apart. Nothing should leak from one into the other.
#[test]
fn two_memory_slots_are_independent() {
    let audio = TrackMemory::default();
    let subtitle = TrackMemory::default();
    remember_track(&audio, Some(TrackPreference::Off));
    assert_eq!(remembered_track(&subtitle), None);
}
