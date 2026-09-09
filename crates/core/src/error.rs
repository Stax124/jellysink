use std::fmt;

/// A mistake in how jellysink was invoked — not logged in, already running, an
/// unknown config key. The binaries print it and exit 1 instead of dumping a
/// color-eyre report.
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
