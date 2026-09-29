pub mod cast;
pub mod config;
pub mod error;
pub mod instance;
pub mod jellyfin;
pub(crate) mod json;
pub mod logging;
pub mod status;
pub mod ticks;
pub mod update;

pub(crate) use error::usage_err;

pub const APP_NAME: &str = "jellysink";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Both binaries speak TLS through the same rustls build, which has no default
/// provider compiled in.
pub fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}
