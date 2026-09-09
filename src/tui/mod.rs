//! jellytui: a terminal frontend that browses Jellyfin and casts to a running
//! jellysink. See `specs/tui.md`.

mod app;
mod keys;
mod nav;
mod ui;

use crate::app::config::{Credentials, Paths};
use crate::jellyfin::auth::Api;
use crate::usage_err;
use color_eyre::eyre::Result;

pub async fn run(paths: Paths) -> Result<()> {
    let credentials = Credentials::load(&paths)?
        .ok_or_else(|| usage_err("not logged in; run `jellysink login` first"))?;
    let api = Api::from_credentials(&credentials)?;
    app::App::new(api, paths).run().await
}
