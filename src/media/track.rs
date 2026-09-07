//! Remembering the track the user picked, and finding it again in the next
//! episode.
//!
//! A choice is remembered as an *identity* ([`TrackId`]) and re-matched against
//! what the next item offers, because stream indexes are per-file and the
//! server's defaults are exactly what the user is overriding. In memory only,
//! most recent selection only.
//!
//! Audio and subtitles share this matcher; [`crate::media::audio`] and
//! [`crate::media::subtitle`] are the two thin sides of it.

/// One selectable stream, identified by what it *is* rather than where it sits,
/// since "index 3" is not the same track twice.
///
/// `is_forced` and `is_external` are subtitle notions; for audio they are
/// always `false`, so they cannot change a ranking.
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
    /// The `kind` field on every log line the two sides share.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Audio => "audio",
            Self::Subtitle => "subtitle",
        }
    }
}

/// The track the user last chose by hand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TrackPreference {
    /// Switched off. Resolves to an explicit `-1`, not "unspecified": for
    /// subtitles, `sub-add` selects what it adds, so unset shows the last one.
    Off,
    /// This track was chosen. Match its equivalent, never its index.
    Stream(TrackId),
}

impl TrackPreference {
    /// What the user just picked, or `None` when it cannot be identified — an
    /// unselectable index, or a stream with neither a language nor a name.
    /// Forgotten rather than stored, so it falls back to the server default
    /// instead of keeping a stale choice alive.
    pub(crate) fn from_selection(candidates: &[TrackId], stream_index: i64) -> Option<Self> {
        // Off is a decision even for an item with no streams of this kind.
        if stream_index < 0 {
            return Some(Self::Off);
        }
        let picked = candidates.iter().find(|c| c.index == stream_index)?;
        is_identifiable(picked).then(|| Self::Stream(picked.clone()))
    }
}

/// Score weights. Non-overlapping magnitudes, so the sum is lexicographic:
/// everything below [`LANGUAGE`] together cannot outrank the language.
const LANGUAGE: u32 = 1000;
/// Neither side names a language. Comparable, but never qualifying on its own.
const BOTH_LANGUAGES_UNKNOWN: u32 = 200;
const TITLE: u32 = 400;
const DISPLAY_TITLE: u32 = 200;
const FORCED: u32 = 40;
const EXTERNAL: u32 = 20;
const CODEC: u32 = 10;
/// A tiebreak only, and strictly weaker than every semantic signal.
const INDEX: u32 = 5;

/// A field's value, or `None` when it carries no identity. Jellyfin sends `""`
/// and `"und"` rather than omitting, and those match everything.
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

/// Both sides present and equal ignoring ASCII case. Two absences never match:
/// nothing is not an identity.
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
    // same track; alone they match every non-forced embedded SRT equally.
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
        // Ties go to the lowest index, so this is stable.
        .min_by_key(|(score, candidate)| (std::cmp::Reverse(*score), candidate.index))
        .map(|(_, candidate)| candidate)
}

/// The Jellyfin stream index to play for this item. Precedence: `requested`
/// (the remote just told us), then a matching remembered preference, then
/// `server_default`.
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
