//! Turning a Jellyfin item into something mpv can play.

pub(crate) mod audio;
pub(crate) mod prepare;
pub(crate) mod streams;
pub(crate) mod subtitle;
pub(crate) mod title;
pub(crate) mod track;

pub(crate) use audio::resolve_audio_index;
pub(crate) use prepare::{PlayRequest, PreparedPlay, prepare_play};
pub(crate) use streams::{
    jellyfin_embedded_audio_index, jellyfin_embedded_subtitle_index, mpv_audio_track_id,
    mpv_embedded_subtitle_track_id,
};
pub(crate) use subtitle::resolve_subtitle_index;
pub(crate) use title::{display_title, episode_titles, item_type, series_id};
pub(crate) use track::{TrackId, TrackKind, TrackPreference};
