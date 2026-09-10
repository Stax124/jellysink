//! Shared fixtures for the test modules under `app/` and `view/`.

use crate::app::App;
use crate::logs::LogBuffer;
use jellysink_core::config::{Credentials, Paths};
use jellysink_core::jellyfin::auth::Api;

pub(crate) fn app() -> App {
    app_with_logs(LogBuffer::new())
}

/// For the tests that have to put lines in the buffer the pane is reading.
pub(crate) fn app_with_logs(logs: LogBuffer) -> App {
    let credentials = Credentials {
        server: "http://localhost:8096".into(),
        username: "test".into(),
        user_id: "u1".into(),
        access_token: "t1".into(),
        device_id: "d1".into(),
    };
    // `main` installs it; a test that builds an `Api` without one panics
    // inside reqwest, because this rustls build has no default provider.
    jellysink_core::install_crypto_provider();
    App::new(
        Api::from_credentials(&credentials).unwrap(),
        Paths::from_override(Some(std::path::PathBuf::from("/nonexistent"))).unwrap(),
        ratatui_image::picker::Picker::halfblocks(),
        logs,
    )
}
