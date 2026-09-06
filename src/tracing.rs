use color_eyre::eyre::{Result, WrapErr};
use tracing_error::ErrorLayer;
use tracing_subscriber::filter::Targets;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Parse a `tracing` filter spec (`info`, `jellysink=debug,warn`, …).
///
/// `Targets`, not `EnvFilter`, to keep the binary small — but not for the
/// reason this comment used to give. `regex`, `regex-automata`, `regex-syntax`
/// and `aho-corasick` are *already* linked in via `self_update`, and
/// `EnvFilter` adds only one crate on top (`matchers`).
///
/// Measured anyway, because the crate count is misleading: enabling
/// `env-filter` grew the release binary from 9,072,344 to 9,258,856 bytes
/// (+186 KB, +2.1%) — LTO drops much of `regex` today, and `EnvFilter` pulls it
/// back. `Targets` covers the directives jellysink actually uses (`log_level`
/// and simple `RUST_LOG` values); what it gives up is span-field filtering.
pub(crate) fn parse_log_filter(spec: &str) -> Result<Targets> {
    spec.parse()
        .wrap_err_with(|| format!("invalid log filter {spec:?}"))
}

/// Validates a `log_level` before it is written to config.toml.
///
/// Stricter than [`parse_log_filter`] on purpose. `Targets` reads a bare word
/// as a *target name*, so `log_level = "banana"` parses happily and then
/// filters out everything jellysink logs — the setting silently does the
/// opposite of what was meant. A spec with no `=` is a level, so require it to
/// be one.
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
#[path = "tracing_test.rs"]
mod tests;
