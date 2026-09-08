use std::fmt;

pub mod app;
pub(crate) mod cast;
pub(crate) mod jellyfin;
pub(crate) mod media;
pub(crate) mod mpv;
pub(crate) mod report;
pub(crate) mod runtime;
pub(crate) mod ticks;

pub(crate) const APP_NAME: &str = "jellysink";
pub(crate) const CLIENT_NAME: &str = "jellysink";
pub(crate) const VERSION: &str = env!("CARGO_PKG_VERSION");

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
