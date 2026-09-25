use std::fmt;

#[derive(Debug)]
pub(crate) struct UsageError(pub String);

impl fmt::Display for UsageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for UsageError {}

pub fn usage_err(msg: impl Into<String>) -> color_eyre::eyre::Report {
    UsageError(msg.into()).into()
}

/// A [`UsageError`] is printed and exits 1 without a color-eyre report.
pub fn exit_on_usage_error(result: color_eyre::Result<()>) -> color_eyre::Result<()> {
    if let Err(err) = &result
        && let Some(usage) = err.downcast_ref::<UsageError>()
    {
        eprintln!("{usage}");
        std::process::exit(1);
    }
    result
}
