//! The Jellyfin endpoints only the daemon calls, over the `Api` client
//! `jellysink_core` owns.

mod playback;
pub(crate) mod profile;

pub(crate) use playback::{playback_info, post_capabilities, post_report};
