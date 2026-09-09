//! The subtitle side of the track memory: names and log `kind` only, with the
//! matching in [`crate::media::track`].

use super::streams::SubtitleId;
use super::track::{TrackKind, TrackPreference, resolve_track_index};

/// The subtitle track the user last chose by hand. [`TrackPreference::Off`] is
/// an explicit `sid=no`, since `sub-add` selects whatever it just added.
pub(crate) type SubtitlePreference = TrackPreference;

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
