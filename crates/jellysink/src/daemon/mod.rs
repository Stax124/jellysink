//! Running as a daemon: the single-instance socket, the desktop integrations
//! and the self-updater. No playback logic lives here.

pub(crate) mod instance;
pub(crate) mod mpris;
pub(crate) mod signal;
pub(crate) mod terminal;
pub(crate) mod tray;
pub(crate) mod update;
