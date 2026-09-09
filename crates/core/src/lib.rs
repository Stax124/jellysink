//! What jellysink and jellytui both need: the config and credential files, the
//! Jellyfin API client, the cast-command vocabulary the two exchange through
//! the server, and the `stop.sock` protocol jellytui polls for its footer.

pub mod cast;
pub mod config;
pub mod error;
pub mod instance;
pub mod jellyfin;
pub mod logging;
pub mod status;
pub mod ticks;

pub use error::{UsageError, usage_err};

pub const APP_NAME: &str = "jellysink";
pub const CLIENT_NAME: &str = "jellysink";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Both binaries speak TLS through the same rustls build, which has no default
/// provider compiled in.
pub fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}
