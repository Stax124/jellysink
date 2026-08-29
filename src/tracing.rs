use color_eyre::eyre::{Result, WrapErr};
use tracing_error::ErrorLayer;
use tracing_subscriber::filter::Targets;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Parse a `tracing` filter spec (`info`, `jellysink=debug,warn`, …).
///
/// `EnvFilter` pulls in `regex-automata`; `Targets` covers the directives we
/// actually use (`log_level` and simple `RUST_LOG` values).
pub fn parse_log_filter(spec: &str) -> Result<Targets> {
    spec.parse()
        .wrap_err_with(|| format!("invalid log filter {spec:?}"))
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
mod tests {
    use super::*;

    #[test]
    fn parse_log_filter_accepts_a_level() {
        let filter = parse_log_filter("debug").unwrap();
        let rendered = filter.to_string();
        assert!(rendered.contains("debug"), "expected debug in {rendered:?}");
    }

    #[test]
    fn parse_log_filter_accepts_target_directives() {
        let filter = parse_log_filter("jellysink=trace,warn").unwrap();
        let rendered = filter.to_string();
        assert!(
            rendered.contains("jellysink"),
            "expected target in {rendered:?}"
        );
    }
}
