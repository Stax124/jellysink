//! The audio side of the track memory.
//!
//! All of the matching lives in [`crate::media::track`]; this is the
//! audio-specific surface — the names the rest of the code uses, and the `kind`
//! the log lines are tagged with.
//!
//! Audio needs this for the same reason subtitles do: a dual-audio release
//! flags whichever track it likes as `DefaultAudioStreamIndex`, so a user who
//! switches from the dub to the original track gets the dub back on the next
//! episode. Stream indexes are per-file, so the choice is remembered as an
//! identity and re-matched.

use super::streams::AudioId;
use super::track::{TrackKind, TrackMemory, TrackPreference, resolve_track_index};

/// The audio track the user last chose by hand.
///
/// [`TrackPreference::Off`] is reachable: mpv's `cycle audio` (`#`) cycles
/// through "no audio", and that is a decision like any other — it resolves to
/// `-1`, an explicit `aid=no`.
pub(crate) type AudioPreference = TrackPreference;

/// The one slot holding the [`AudioPreference`]. See [`TrackMemory`].
pub(crate) type AudioMemory = TrackMemory;

/// The Jellyfin audio stream index to play for this item.
///
/// See [`resolve_track_index`] for the precedence.
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
