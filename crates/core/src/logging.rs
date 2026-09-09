use color_eyre::eyre::{Result, WrapErr};
use tracing_error::ErrorLayer;
use tracing_subscriber::filter::Targets;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Parse a `tracing` filter spec (`info`, `jellysink=debug,warn`, …).
///
/// `Targets` rather than `EnvFilter`: measured at +186 KB (+2.1%) of release
/// binary, and all we give up is span-field filtering.
pub(crate) fn parse_log_filter(spec: &str) -> Result<Targets> {
    spec.parse()
        .wrap_err_with(|| format!("invalid log filter {spec:?}"))
}

/// Validates a `log_level` before it is written to config.toml. Stricter than
/// [`parse_log_filter`], which reads a bare `"banana"` as a target name and
/// then silently filters out everything jellysink logs.
pub(crate) fn validate_log_level(spec: &str) -> Result<()> {
    parse_log_filter(spec)?;
    let bare = spec.trim();
    if !bare.contains('=') && !bare.contains(',') {
        const LEVELS: [&str; 6] = ["trace", "debug", "info", "warn", "error", "off"];
        if !LEVELS.iter().any(|l| l.eq_ignore_ascii_case(bare)) {
            return Err(color_eyre::eyre::eyre!(
                "expected one of {} (or a target filter like `jellysink=debug,warn`)",
                LEVELS.join(", ")
            ));
        }
    }
    Ok(())
}

fn log_filter(level: &str) -> Result<Targets> {
    match std::env::var("RUST_LOG") {
        Ok(spec) => parse_log_filter(&spec),
        Err(_) => parse_log_filter(level),
    }
}

pub fn init_tracing(level: &str) -> Result<()> {
    let filter = log_filter(level)?;
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().without_time())
        .with(ErrorLayer::default())
        .init();
    color_eyre::install()?;
    Ok(())
}

#[cfg(test)]
#[path = "logging_test.rs"]
mod tests;
