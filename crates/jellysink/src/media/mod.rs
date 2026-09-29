//! Turning a Jellyfin item into something mpv can play.

pub(crate) mod prepare;
pub(crate) mod streams;
pub(crate) mod title;
pub(crate) mod track;

pub(crate) use prepare::{PlayRequest, PreparedPlay, prepare_play};
pub(crate) use title::{display_title, episode_titles, item_type, series_id};
pub(crate) use track::{TrackKind, TrackPreference, resolve_track_index};
