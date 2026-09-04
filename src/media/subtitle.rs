//! Remembering the subtitle track the user picked, and finding it again in the
//! next episode.
//!
//! Jellyfin stream indexes are per-file, so remembering the index is useless:
//! the next episode can order its streams differently, or come from a different
//! provider. And the server's `DefaultSubtitleStreamIndex` is what we are
//! working around in the first place — it points at the wrong track for
//! mislabeled releases, and for releases that split one language into
//! `Signs and Songs` and `Dialogue` it regularly picks the wrong half.
//!
//! So a choice is remembered as an *identity* ([`SubtitleId`]) and re-matched
//! against whatever the next item actually offers. Nothing here reaches disk:
//! the preference lives in memory for as long as the daemon runs and holds only
//! the most recent selection.

use super::streams::SubtitleId;
use std::sync::{Arc, Mutex, PoisonError};

/// The subtitle track the user last chose by hand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SubtitlePreference {
    /// Subtitles were switched off. Resolves to `-1`, an explicit `sid=no`
    /// rather than "unspecified" — it has to, because `sub-add` selects the
    /// track it adds, so leaving the index unset shows the last external
    /// subtitle instead of none.
    Off,
    /// This track was chosen. Match its equivalent, never its index.
    Stream(SubtitleId),
}

/// The one slot holding the [`SubtitlePreference`].
///
/// Shared rather than owned because `Runtime` is rebuilt by `run_session` on
/// every websocket reconnect; a plain field would drop the choice on any
/// network blip. Created once in `runtime::run`, cloned into each session.
pub(crate) type SubtitleMemory = Arc<Mutex<Option<SubtitlePreference>>>;

/// Reads the remembered choice out of the shared slot.
///
/// Clones rather than handing back a guard: every caller is `async`, and a
/// `std::sync::MutexGuard` held across an `.await` is exactly the deadlock this
/// avoids. A poisoned lock still yields the value — losing a subtitle
/// preference is not worth a panic in a daemon.
pub(crate) fn remembered_subtitle_preference(
    memory: &SubtitleMemory,
) -> Option<SubtitlePreference> {
    memory
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// Overwrites the remembered choice. `None` forgets it.
pub(crate) fn remember_subtitle_preference(
    memory: &SubtitleMemory,
    preference: Option<SubtitlePreference>,
) {
    *memory.lock().unwrap_or_else(PoisonError::into_inner) = preference;
}

impl SubtitlePreference {
    /// What the user just picked, or `None` when it cannot be identified.
    ///
    /// Two choices are unidentifiable: an index this item cannot select, and a
    /// stream carrying neither a language nor a name. Both are *forgotten*
    /// rather than stored — an identity that can never match again would just
    /// keep the previous choice alive, and silently re-applying a track the
    /// user has already moved away from is the most confusing outcome
    /// available.
    pub(crate) fn from_selection(candidates: &[SubtitleId], stream_index: i64) -> Option<Self> {
        // Off is a decision even for an item with no subtitle streams at all,
        // so it never consults the candidate list.
        if stream_index < 0 {
            return Some(Self::Off);
        }
        let picked = candidates.iter().find(|c| c.index == stream_index)?;
        is_identifiable(picked).then(|| Self::Stream(picked.clone()))
    }
}

/// Score weights.
///
/// The magnitudes are deliberately non-overlapping, so the sum behaves
/// lexicographically: everything below [`LANGUAGE`] adds up to less than it, so
/// no pile of name and flag agreements can ever outrank the language. That
/// ordering is the point — playing the wrong language is a far worse failure
/// than playing the wrong track within the right one.
const LANGUAGE: u32 = 1000;
/// Neither side names a language. Worth something (two unlabelled tracks in a
/// single-language release really are comparable) but never enough to qualify a
/// candidate on its own.
const BOTH_LANGUAGES_UNKNOWN: u32 = 200;
const TITLE: u32 = 400;
const DISPLAY_TITLE: u32 = 200;
const FORCED: u32 = 40;
const EXTERNAL: u32 = 20;
const CODEC: u32 = 10;
/// A tiebreak only, and strictly weaker than every semantic signal.
const INDEX: u32 = 5;

/// A field's value, or `None` when it carries no identity.
///
/// Jellyfin sends `""` and `"und"` rather than omitting these, and treating
/// those as a value would make every unlabelled track match every other one.
fn named(field: &Option<String>) -> Option<&str> {
    let value = field.as_deref()?.trim();
    if value.is_empty()
        || value.eq_ignore_ascii_case("und")
        || value.eq_ignore_ascii_case("undefined")
    {
        return None;
    }
    Some(value)
}

/// Both sides present and equal ignoring ASCII case. Two absences are never a
/// match: nothing is not an identity.
///
/// `eq_ignore_ascii_case` rather than `to_lowercase` so matching does not
/// allocate a `String` per field per candidate per episode; a title with no
/// ASCII in it has no case to fold anyway.
fn same(a: Option<&str>, b: Option<&str>) -> bool {
    matches!((a, b), (Some(a), Some(b)) if a.eq_ignore_ascii_case(b))
}

/// Whether a stream has anything that could identify it in another item.
fn is_identifiable(id: &SubtitleId) -> bool {
    named(&id.language).is_some()
        || named(&id.title).is_some()
        || named(&id.display_title).is_some()
}

/// How well `candidate` matches `wanted`, or `None` when it is not
/// recognisably the same track.
fn score(wanted: &SubtitleId, candidate: &SubtitleId) -> Option<u32> {
    let wanted_language = named(&wanted.language);
    let candidate_language = named(&candidate.language);
    let language = same(wanted_language, candidate_language);
    let title = same(named(&wanted.title), named(&candidate.title));
    let display_title = same(
        named(&wanted.display_title),
        named(&candidate.display_title),
    );

    // Flags, codec and index only *rank* candidates that already look like the
    // same track. On their own they would happily match an unrelated stream —
    // every non-forced embedded SRT agrees with every other one.
    if !(language || title || display_title) {
        return None;
    }

    let mut total = 0;
    if language {
        total += LANGUAGE;
    } else if wanted_language.is_none() && candidate_language.is_none() {
        total += BOTH_LANGUAGES_UNKNOWN;
    }
    if title {
        total += TITLE;
    }
    if display_title {
        total += DISPLAY_TITLE;
    }
    if wanted.is_forced == candidate.is_forced {
        total += FORCED;
    }
    if wanted.is_external == candidate.is_external {
        total += EXTERNAL;
    }
    if same(named(&wanted.codec), named(&candidate.codec)) {
        total += CODEC;
    }
    if wanted.index == candidate.index {
        total += INDEX;
    }
    Some(total)
}

/// The candidate that best matches `wanted`, or `None` when nothing qualifies.
fn best_match<'a>(wanted: &SubtitleId, candidates: &'a [SubtitleId]) -> Option<&'a SubtitleId> {
    candidates
        .iter()
        .filter_map(|candidate| Some((score(wanted, candidate)?, candidate)))
        // Highest score wins; a tie goes to the lowest index, so an item with
        // two indistinguishable tracks resolves the same way every time.
        .min_by_key(|(score, candidate)| (std::cmp::Reverse(*score), candidate.index))
        .map(|(_, candidate)| candidate)
}

/// The Jellyfin subtitle stream index to play for this item.
///
/// Precedence, highest first:
///
/// 1. `requested` — the remote named a stream for this item. It just told us
///    what the user wants; nothing we remember outranks that.
/// 2. The remembered preference, when this item has a track matching it.
/// 3. `server_default` — `DefaultSubtitleStreamIndex`, the behaviour before any
///    of this existed.
pub(crate) fn resolve_subtitle_index(
    requested: Option<i64>,
    preference: Option<&SubtitlePreference>,
    candidates: &[SubtitleId],
    server_default: Option<i64>,
) -> Option<i64> {
    if let Some(requested) = requested {
        return Some(requested);
    }
    match preference {
        None => server_default,
        Some(SubtitlePreference::Off) => {
            tracing::info!(server_default, "keeping subtitles off; remembered choice");
            Some(-1)
        }
        Some(SubtitlePreference::Stream(wanted)) => match best_match(wanted, candidates) {
            Some(found) => {
                tracing::info!(
                    remembered_index = wanted.index,
                    matched_index = found.index,
                    language = found.language.as_deref(),
                    title = found.title.as_deref(),
                    display_title = found.display_title.as_deref(),
                    server_default,
                    "applied remembered subtitle"
                );
                Some(found.index)
            }
            None => {
                tracing::debug!(
                    remembered_index = wanted.index,
                    remembered_language = wanted.language.as_deref(),
                    remembered_title = wanted.title.as_deref(),
                    candidates = candidates.len(),
                    server_default,
                    "nothing here matches the remembered subtitle; using the server default"
                );
                server_default
            }
        },
    }
}

#[cfg(test)]
mod tests {
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
    fn the_right_language_outranks_the_right_track_name_in_the_wrong_language() {
        let wanted = sub(2, "eng", "Signs");
        let candidates = vec![sub(2, "spa", "Signs"), sub(3, "eng", "Dialogue")];
        assert_eq!(best_match(&wanted, &candidates).map(|c| c.index), Some(3));
    }

    #[test]
    fn the_display_title_matches_when_the_provider_supplies_no_track_title() {
        let named_only_by_display = |index: i64, display: &str| SubtitleId {
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
        let unnamed = |index: i64, is_forced: bool| SubtitleId {
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
        let wanted = sub(2, "eng", "Dialogue");
        let candidates = vec![sub(7, "jpn", "Full"), sub(8, "eng", "Full Subtitles")];
        assert_eq!(best_match(&wanted, &candidates).map(|c| c.index), Some(8));
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

    #[test]
    fn matching_ignores_case_and_surrounding_whitespace() {
        let wanted = sub(1, "ENG", "  Dialogue ");
        let candidates = vec![sub(4, "eng", "dialogue")];
        assert_eq!(best_match(&wanted, &candidates).map(|c| c.index), Some(4));
    }

    #[test]
    fn an_empty_or_undefined_language_is_not_a_language_match() {
        // Jellyfin sends "" and "und" rather than omitting the field. Treating
        // either as a value would match every unlabelled track to every other.
        for language in ["", "und", "  "] {
            let wanted = SubtitleId {
                index: 1,
                language: Some(language.into()),
                ..Default::default()
            };
            let candidates = vec![SubtitleId {
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
        let wanted = sub(1, "eng", "Dialogue");
        // Same flags, same codec, same index — and nothing in common that means
        // anything.
        let candidates = vec![sub(1, "jpn", "Signs")];
        assert!(best_match(&wanted, &candidates).is_none());
    }

    #[test]
    fn a_stream_with_no_language_or_name_cannot_be_remembered() {
        let candidates = vec![SubtitleId {
            index: 3,
            ..Default::default()
        }];
        assert_eq!(SubtitlePreference::from_selection(&candidates, 3), None);
    }

    #[test]
    fn an_index_the_item_cannot_select_cannot_be_remembered() {
        let candidates = signs_and_dialogue(2, 3);
        assert_eq!(SubtitlePreference::from_selection(&candidates, 9), None);
    }

    #[test]
    fn a_selectable_named_stream_is_remembered_whole() {
        let candidates = signs_and_dialogue(2, 3);
        assert_eq!(
            SubtitlePreference::from_selection(&candidates, 3),
            Some(SubtitlePreference::Stream(sub(3, "eng", "Dialogue")))
        );
    }

    #[test]
    fn the_lowest_index_wins_between_two_indistinguishable_tracks() {
        let wanted = sub(9, "eng", "Dialogue");
        let candidates = vec![sub(5, "eng", "Dialogue"), sub(4, "eng", "Dialogue")];
        assert_eq!(best_match(&wanted, &candidates).map(|c| c.index), Some(4));
    }

    #[test]
    fn the_shared_memory_round_trips_and_can_be_forgotten() {
        let memory = SubtitleMemory::default();
        assert_eq!(remembered_subtitle_preference(&memory), None);
        remember_subtitle_preference(&memory, Some(SubtitlePreference::Off));
        assert_eq!(
            remembered_subtitle_preference(&memory),
            Some(SubtitlePreference::Off)
        );
        // A second session holding the same slot sees the choice; this is what
        // survives a websocket reconnect.
        assert_eq!(
            remembered_subtitle_preference(&SubtitleMemory::clone(&memory)),
            Some(SubtitlePreference::Off)
        );
        remember_subtitle_preference(&memory, None);
        assert_eq!(remembered_subtitle_preference(&memory), None);
    }
}
