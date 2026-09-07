//! The audio side of the track memory: names and log `kind` only, with the
//! matching in [`crate::media::track`].
//!
//! Needed because a dual-audio release flags whichever track it likes as
//! `DefaultAudioStreamIndex`, so a user who picks the original gets the dub
//! back next episode.

use super::streams::AudioId;
use super::track::{TrackKind, TrackMemory, TrackPreference, resolve_track_index};

/// The audio track the user last chose by hand. [`TrackPreference::Off`] is
/// reachable: `cycle audio` (`#`) passes through "no audio".
pub(crate) type AudioPreference = TrackPreference;

/// The one slot holding the [`AudioPreference`]. See [`TrackMemory`].
pub(crate) type AudioMemory = TrackMemory;

/// The Jellyfin audio stream index to play; see [`resolve_track_index`].
pub(crate) fn resolve_audio_index(
    requested: Option<i64>,
    preference: Option<&AudioPreference>,
    candidates: &[AudioId],
    server_default: Option<i64>,
) -> Option<i64> {
    resolve_track_index(
        TrackKind::Audio,
        requested,
        preference,
        candidates,
        server_default,
    )
}

#[cfg(test)]
#[path = "audio_test.rs"]
mod tests;
