//! Running as a daemon: the control socket and the desktop
//! integrations. No playback logic lives here.

pub(crate) mod control;
pub(crate) mod mpris;
pub(crate) mod signal;
pub(crate) mod terminal;
pub(crate) mod tray;
