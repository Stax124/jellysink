//! jellytui: a terminal frontend that browses Jellyfin and casts to a running
//! jellysink. See `specs/tui.md`.

mod app;
mod cover;
mod grid;
mod keys;
mod nav;
mod playing;
mod rail;
mod ui;

use crate::app::config::{Config, Credentials, Paths};
use crate::jellyfin::auth::Api;
use crate::usage_err;
use color_eyre::eyre::Result;

pub async fn run(paths: Paths) -> Result<()> {
    let credentials = Credentials::load(&paths)?
        .ok_or_else(|| usage_err("not logged in; run `jellysink login` first"))?;
    let api = Api::from_credentials(&credentials)?;
    // Before the alternate screen is taken: the protocol query writes to
    // stdout and reads the terminal's answer back off stdin.
    let picker = cover::detect_picker();
    let image_scale = Config::load(&paths)?.image_scale;
    app::App::new(api, paths, picker, image_scale).run().await
}
