use std::fmt;

pub mod cast;
pub mod cli;
pub mod config;
pub mod instance;
pub mod jellyfin;
pub mod media;
pub mod mpv;
pub mod report;
pub mod runtime;
pub mod terminal;
pub mod tracing;
pub mod tray;
pub mod update;

pub const APP_NAME: &str = "jellysink";
pub const CLIENT_NAME: &str = "jellysink";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug)]
pub struct UsageError(pub String);

impl fmt::Display for UsageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for UsageError {}

pub fn usage_err(msg: impl Into<String>) -> color_eyre::eyre::Report {
    UsageError(msg.into()).into()
}
