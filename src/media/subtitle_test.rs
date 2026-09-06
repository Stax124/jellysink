use super::*;

/// The fields that carry identity; the rest come from `Default`.
fn sub(index: i64, language: &str, title: &str) -> SubtitleId {
    SubtitleId {
        index,
        language: Some(language.to_string()),
        title: Some(title.to_string()),
        ..Default::default()
    }
}

/// The case this whole module exists for: one language, two tracks, and the
/// indexes swapped between episodes.
fn signs_and_dialogue(signs: i64, dialogue: i64) -> Vec<SubtitleId> {
    vec![
        sub(signs, "eng", "Signs and Songs"),
        sub(dialogue, "eng", "Dialogue"),
    ]
}

#[test]
fn a_remembered_track_is_matched_by_language_and_name_not_by_index() {
    let wanted = SubtitlePreference::Stream(sub(2, "eng", "Dialogue"));
    // The next episode numbers the same two tracks the other way round.
    let next = signs_and_dialogue(2, 3);
    assert_eq!(
        resolve_subtitle_index(None, Some(&wanted), &next, Some(2)),
        Some(3)
    );
}

#[test]
fn the_servers_signs_and_songs_default_loses_to_the_remembered_dialogue_track() {
    let wanted = SubtitlePreference::Stream(sub(3, "eng", "Dialogue"));
    let next = signs_and_dialogue(2, 3);
    // The server insists on Signs and Songs; the remembered choice wins.
    assert_eq!(
        resolve_subtitle_index(None, Some(&wanted), &next, Some(2)),
        Some(3)
    );
}

#[test]
fn a_language_the_next_episode_does_not_have_falls_back_to_the_server_default() {
    let wanted = SubtitlePreference::Stream(sub(2, "ces", "Dialogue"));
    let candidates = vec![sub(1, "jpn", "Signs"), sub(2, "spa", "Completos")];
    assert_eq!(
        resolve_subtitle_index(None, Some(&wanted), &candidates, Some(1)),
        Some(1)
    );
}

#[test]
fn an_explicit_index_from_the_remote_beats_the_remembered_track() {
    let wanted = SubtitlePreference::Stream(sub(3, "eng", "Dialogue"));
    let candidates = signs_and_dialogue(2, 3);
    assert_eq!(
        resolve_subtitle_index(Some(2), Some(&wanted), &candidates, Some(3)),
        Some(2),
        "the remote just said what it wants for this item"
    );
}

#[test]
fn off_is_remembered_and_forces_minus_one_over_a_server_default() {
    let candidates = signs_and_dialogue(2, 3);
    assert_eq!(
        resolve_subtitle_index(None, Some(&SubtitlePreference::Off), &candidates, Some(2)),
        Some(-1)
    );
}

#[test]
fn off_still_resolves_when_the_next_episode_has_no_subtitles_at_all() {
    assert_eq!(
        SubtitlePreference::from_selection(&[], -1),
        Some(SubtitlePreference::Off)
    );
    assert_eq!(
        resolve_subtitle_index(None, Some(&SubtitlePreference::Off), &[], None),
        Some(-1)
    );
}

#[test]
fn no_preference_leaves_the_server_default_untouched() {
    let candidates = signs_and_dialogue(2, 3);
    assert_eq!(
        resolve_subtitle_index(None, None, &candidates, Some(2)),
        Some(2)
    );
    assert_eq!(resolve_subtitle_index(None, None, &candidates, None), None);
    assert_eq!(
        resolve_subtitle_index(None, None, &candidates, Some(-1)),
        Some(-1)
    );
}
