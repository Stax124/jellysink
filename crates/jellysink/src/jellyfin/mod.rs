//! The Jellyfin endpoints only the daemon calls, over the `Api` client
//! `jellysink_core` owns.

mod playback;
pub(crate) mod profile;

pub(crate) use playback::{playback_info, playing, post_capabilities, progress, stopped};
