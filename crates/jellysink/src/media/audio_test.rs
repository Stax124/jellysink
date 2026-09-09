use super::*;

/// The fields that carry identity; the rest come from `Default`.
fn audio(index: i64, language: &str, title: &str) -> AudioId {
    AudioId {
        index,
        language: Some(language.to_string()),
        title: Some(title.to_string()),
        ..Default::default()
    }
}

/// The case this exists for: a dual-audio release whose two tracks swap
/// indexes between episodes.
fn dub_and_original(dub: i64, original: i64) -> Vec<AudioId> {
    vec![
        audio(dub, "eng", "English Dub"),
        audio(original, "jpn", "Original"),
    ]
}

#[test]
fn a_remembered_track_is_matched_by_language_and_name_not_by_index() {
    let wanted = AudioPreference::Stream(audio(1, "jpn", "Original"));
    // The next episode numbers the same two tracks the other way round.
    let next = dub_and_original(1, 2);
    assert_eq!(
        resolve_audio_index(None, Some(&wanted), &next, Some(1)),
        Some(2)
    );
}

#[test]
fn the_servers_default_dub_loses_to_the_remembered_original_track() {
    let wanted = AudioPreference::Stream(audio(2, "jpn", "Original"));
    let next = dub_and_original(1, 2);
    // The server flags the dub as default; the remembered choice wins.
    assert_eq!(
        resolve_audio_index(None, Some(&wanted), &next, Some(1)),
        Some(2)
    );
}

/// A commentary track and the feature audio share a language, so the track
/// name is the only thing telling them apart.
#[test]
fn a_commentary_track_is_not_confused_with_the_feature_audio() {
    let wanted = AudioPreference::Stream(audio(1, "eng", "Commentary"));
    let candidates = vec![
        audio(3, "eng", "Surround 5.1"),
        audio(4, "eng", "Commentary"),
    ];
    assert_eq!(
        resolve_audio_index(None, Some(&wanted), &candidates, Some(3)),
        Some(4)
    );
}

#[test]
fn a_language_the_next_episode_does_not_have_falls_back_to_the_server_default() {
    let wanted = AudioPreference::Stream(audio(2, "ces", "Dabing"));
    let candidates = dub_and_original(1, 2);
    assert_eq!(
        resolve_audio_index(None, Some(&wanted), &candidates, Some(1)),
        Some(1)
    );
}

#[test]
fn an_explicit_index_from_the_remote_beats_the_remembered_track() {
    let wanted = AudioPreference::Stream(audio(2, "jpn", "Original"));
    let candidates = dub_and_original(1, 2);
    assert_eq!(
        resolve_audio_index(Some(1), Some(&wanted), &candidates, Some(2)),
        Some(1),
        "the remote just said what it wants for this item"
    );
}

/// `#` in the mpv window cycles through "no audio"; that is a decision, and
/// it has to survive into the next episode like any other.
#[test]
fn off_is_remembered_and_forces_minus_one_over_a_server_default() {
    let candidates = dub_and_original(1, 2);
    assert_eq!(
        resolve_audio_index(None, Some(&AudioPreference::Off), &candidates, Some(1)),
        Some(-1)
    );
}

#[test]
fn no_preference_leaves_the_server_default_untouched() {
    let candidates = dub_and_original(1, 2);
    assert_eq!(
        resolve_audio_index(None, None, &candidates, Some(2)),
        Some(2)
    );
    assert_eq!(resolve_audio_index(None, None, &candidates, None), None);
}
