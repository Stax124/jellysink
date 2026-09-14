use color_eyre::eyre::Result;
use tracing::level_filters::LevelFilter;
use tracing_error::ErrorLayer;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

pub fn init_tracing() -> Result<()> {
    tracing_subscriber::registry()
        .with(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::INFO.into())
                .from_env_lossy(),
        )
        .with(tracing_subscriber::fmt::layer().without_time())
        .with(ErrorLayer::default())
        .init();
    color_eyre::install()?;
    Ok(())
}
