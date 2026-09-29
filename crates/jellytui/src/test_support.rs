//! Shared fixtures for the test modules.

use crate::app::App;
use crate::cover::{CoverDisk, Covers};
use crate::logs::LogBuffer;
use jellysink_core::config::{Credentials, Paths};
use jellysink_core::jellyfin::auth::Api;
use jellysink_core::jellyfin::model::Item;
use jellysink_core::status::{NowPlaying, PlayerStatus};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Size;
use ratatui::{Frame, Terminal};
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;
use serde::Deserialize;

pub(crate) fn app() -> App {
    app_with_logs(LogBuffer::new())
}

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
        Picker::halfblocks(),
        CoverDisk::disabled(),
        logs,
    )
}

pub(crate) fn covers() -> Covers {
    Covers::new(Picker::halfblocks(), CoverDisk::disabled())
}

pub(crate) fn protocol() -> Protocol {
    Picker::halfblocks()
        .new_protocol(
            image::DynamicImage::new_rgb8(4, 4),
            Size::new(2, 2),
            ratatui_image::Resize::Fit(None),
        )
        .unwrap()
}

pub(crate) fn item(raw: serde_json::Value) -> Item {
    Item::deserialize(raw).unwrap()
}

/// A daemon playing `item_id` a minute into a 23:42 episode, third of 103.
pub(crate) fn playing(item_id: &str, title: &str) -> PlayerStatus {
    PlayerStatus {
        server: "s".into(),
        username: "u".into(),
        now_playing: Some(NowPlaying {
            item_id: item_id.into(),
            title: title.into(),
            position_ticks: 600_000_000,
            run_time_ticks: Some(14_220_809_999),
            is_paused: false,
            is_muted: false,
            volume: 70,
            has_next: true,
            has_previous: true,
            queue_index: 2,
            queue_len: 103,
            art_url: String::new(),
        }),
    }
}

pub(crate) fn render_buffer(width: u16, height: u16, render: impl FnOnce(&mut Frame)) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(render).unwrap();
    terminal.backend().buffer().clone()
}

/// The drawn screen as text, one line per row.
pub(crate) fn drawn(width: u16, height: u16, render: impl FnOnce(&mut Frame)) -> String {
    render_buffer(width, height, render)
        .content()
        .chunks(usize::from(width))
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}
