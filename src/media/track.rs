//! Remembering the track the user picked, and finding it again in the next
//! episode.
//!
//! Jellyfin stream indexes are per-file, so remembering the index is useless:
//! the next episode can order its streams differently, or come from a different
//! provider. And the server's `DefaultAudioStreamIndex` /
//! `DefaultSubtitleStreamIndex` is what we are working around in the first
//! place — it points at the wrong track for mislabeled releases, and for
//! releases that split one language into `Signs and Songs` and `Dialogue` it
//! regularly picks the wrong half.
//!
//! So a choice is remembered as an *identity* ([`TrackId`]) and re-matched
//! against whatever the next item actually offers. Nothing here reaches disk:
//! the preference lives in memory for as long as the daemon runs and holds only
//! the most recent selection.
//!
//! Audio and subtitles differ only in which mpv property carries the selection
//! and in how the log lines read, so both go through this one matcher; see
//! [`crate::media::audio`] and [`crate::media::subtitle`] for the two sides.

/// One selectable stream, identified by what it *is* rather than where it sits.
///
/// Stream indexes are per-file: the next episode can order its streams
/// differently, or come from a different provider, so "index 3" is not the same
/// track twice. Releases that split one language into `Signs and Songs` and
/// `Dialogue` also flag the wrong one as the server default often enough that
/// the index the server hands back is not trustworthy either.
///
/// `is_forced` and `is_external` are subtitle notions; for audio they are
/// always `false` (an external audio stream has no mpv track and never becomes
/// an identity), so their weights add the same constant to every audio
/// candidate and cannot change a ranking.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TrackId {
    pub(crate) index: i64,
    pub(crate) language: Option<String>,
    pub(crate) title: Option<String>,
    pub(crate) display_title: Option<String>,
    pub(crate) codec: Option<String>,
    pub(crate) is_forced: bool,
    pub(crate) is_external: bool,
}

/// Which kind of track a resolution is about. Log wording only — the matching
/// is identical for both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrackKind {
    Audio,
    Subtitle,
}

impl TrackKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Audio => "audio",
            Self::Subtitle => "subtitle",
        }
    }
}

/// The track the user last chose by hand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TrackPreference {
    /// The track was switched off. Resolves to `-1`, an explicit `no` rather
    /// than "unspecified" — for subtitles it has to, because `sub-add` selects
    /// the track it adds, so leaving the index unset shows the last external
    /// subtitle instead of none.
    Off,
    /// This track was chosen. Match its equivalent, never its index.
    Stream(TrackId),
}

/// The one slot holding a [`TrackPreference`]; `None` is "nothing remembered".
///
/// A plain field on `Runtime`, which outlives every websocket session — this
/// used to be an `Arc<Mutex<_>>` shared with `runtime::run`, back when a
/// reconnect built a fresh `Runtime` and a plain field would have dropped the
/// user's choice on any network blip.
///
/// There is one slot per kind. They are the same type, so only the two named
/// `Runtime` fields keep them apart.
pub(crate) type TrackMemory = Option<TrackPreference>;

impl TrackPreference {
    /// What the user just picked, or `None` when it cannot be identified.
    ///
    /// Two choices are unidentifiable: an index this item cannot select, and a
    /// stream carrying neither a language nor a name. Both are *forgotten*
    /// rather than stored — an identity that can never match again would just
    /// keep the previous choice alive, and silently re-applying a track the
    /// user has already moved away from is the most confusing outcome
    /// available.
    pub(crate) fn from_selection(candidates: &[TrackId], stream_index: i64) -> Option<Self> {
        // Off is a decision even for an item with no streams of this kind at
        // all, so it never consults the candidate list.
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
fn is_identifiable(id: &TrackId) -> bool {
    named(&id.language).is_some()
        || named(&id.title).is_some()
        || named(&id.display_title).is_some()
}

/// How well `candidate` matches `wanted`, or `None` when it is not
/// recognisably the same track.
fn score(wanted: &TrackId, candidate: &TrackId) -> Option<u32> {
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
pub(crate) fn best_match<'a>(wanted: &TrackId, candidates: &'a [TrackId]) -> Option<&'a TrackId> {
    candidates
        .iter()
        .filter_map(|candidate| Some((score(wanted, candidate)?, candidate)))
        // Highest score wins; a tie goes to the lowest index, so an item with
        // two indistinguishable tracks resolves the same way every time.
        .min_by_key(|(score, candidate)| (std::cmp::Reverse(*score), candidate.index))
        .map(|(_, candidate)| candidate)
}

/// The Jellyfin stream index to play for this item.
///
/// Precedence, highest first:
///
/// 1. `requested` — the remote named a stream for this item. It just told us
///    what the user wants; nothing we remember outranks that.
/// 2. The remembered preference, when this item has a track matching it.
/// 3. `server_default` — `DefaultAudioStreamIndex` /
///    `DefaultSubtitleStreamIndex`, the behaviour before any of this existed.
pub(crate) fn resolve_track_index(
    kind: TrackKind,
    requested: Option<i64>,
    preference: Option<&TrackPreference>,
    candidates: &[TrackId],
    server_default: Option<i64>,
) -> Option<i64> {
    if let Some(requested) = requested {
        return Some(requested);
    }
    match preference {
        None => server_default,
        Some(TrackPreference::Off) => {
            tracing::info!(
                kind = kind.as_str(),
                server_default,
                "keeping the track off; remembered choice"
            );
            Some(-1)
        }
        Some(TrackPreference::Stream(wanted)) => match best_match(wanted, candidates) {
            Some(found) => {
                tracing::info!(
                    kind = kind.as_str(),
                    remembered_index = wanted.index,
                    matched_index = found.index,
                    language = found.language.as_deref(),
                    title = found.title.as_deref(),
                    display_title = found.display_title.as_deref(),
                    server_default,
                    "applied remembered track"
                );
                Some(found.index)
            }
            None => {
                tracing::debug!(
                    kind = kind.as_str(),
                    remembered_index = wanted.index,
                    remembered_language = wanted.language.as_deref(),
                    remembered_title = wanted.title.as_deref(),
                    candidates = candidates.len(),
                    server_default,
                    "nothing here matches the remembered track; using the server default"
                );
                server_default
            }
        },
    }
}

#[cfg(test)]
#[path = "track_test.rs"]
mod tests;
