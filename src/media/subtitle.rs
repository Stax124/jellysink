//! The subtitle side of the track memory.
//!
//! All of the matching lives in [`crate::media::track`]; this is the
//! subtitle-specific surface — the names the rest of the code uses, and the
//! `kind` the log lines are tagged with.

use super::streams::SubtitleId;
use super::track::{TrackKind, TrackMemory, TrackPreference, resolve_track_index};

/// The subtitle track the user last chose by hand.
///
/// [`TrackPreference::Off`] resolves to `-1`, an explicit `sid=no` rather than
/// "unspecified" — it has to, because `sub-add` selects the track it adds, so
/// leaving the index unset shows the last external subtitle instead of none.
pub(crate) type SubtitlePreference = TrackPreference;

/// The one slot holding the [`SubtitlePreference`]. See [`TrackMemory`].
pub(crate) type SubtitleMemory = TrackMemory;

/// The Jellyfin subtitle stream index to play for this item.
///
/// See [`resolve_track_index`] for the precedence.
pub(crate) fn resolve_subtitle_index(
    requested: Option<i64>,
    preference: Option<&SubtitlePreference>,
    candidates: &[SubtitleId],
    server_default: Option<i64>,
) -> Option<i64> {
    resolve_track_index(
        TrackKind::Subtitle,
        requested,
        preference,
        candidates,
        server_default,
    )
}

#[cfg(test)]
#[path = "subtitle_test.rs"]
mod tests;
