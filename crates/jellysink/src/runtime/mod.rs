mod loading;
mod playback;
mod queue;
mod session;
mod state;
pub(crate) mod task;
mod window;

pub(crate) use session::{CastCommand, run};

#[cfg(test)]
#[path = "jellyfin_integration_test.rs"]
mod jellyfin_integration_tests;
