//! App state and the event loop.

mod browse;
mod msg;
mod player;
mod request;

pub(crate) use browse::Shelf;
use msg::Msg;

use crate::cover::{self, CoverKey, Covers};
use crate::keys::{self, Intent};
use crate::nav::{self, End, Level, Source};
use crate::view::{grid, playing, rail};

use crate::view;
use color_eyre::eyre::Result;
use jellysink_core::config::Paths;
use jellysink_core::instance;
use jellysink_core::jellyfin::auth::Api;
use jellysink_core::jellyfin::browse::{EPISODE_LIMIT, ItemQuery};
use jellysink_core::jellyfin::model::{Item, ItemList};
use jellysink_core::jellyfin::remote::PlaystateCommand;
use jellysink_core::status::PlayerStatus;
use jellysink_core::ticks::seconds_to_ticks;
use ratatui::backend::Backend;
use ratatui::crossterm::event::{Event, KeyEventKind};
use ratatui::layout::{Rect, Size};
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

const HOME_ROWS: u32 = 24;
const SEARCH_LIMIT: u32 = 60;
const PAGE_JUMP: isize = 10;
/// Long enough that typing a word is one request, short enough to feel live.
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(250);
const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// Long enough that holding `j` through a library does not fetch a cover per
/// row, short enough that resting on one shows its art at once.
const COVER_DEBOUNCE: Duration = Duration::from_millis(120);
const SEARCH_TYPES: &str = "Movie,Series,Episode";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Screen {
    Home,
    Browse,
    Search,
    Playing,
}

/// Which of the two Home shelves has focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HomePane {
    Resume,
    NextUp,
}

impl HomePane {
    pub(crate) const ALL: [Self; 2] = [Self::Resume, Self::NextUp];

    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Resume => "Continue Watching",
            Self::NextUp => "Next Up",
        }
    }
}

pub(crate) struct App {
    api: Api,
    tx: UnboundedSender<Msg>,
    rx: UnboundedReceiver<Msg>,
    pub(crate) screen: Screen,
    pub(crate) home_pane: HomePane,
    pub(crate) resume: Shelf,
    pub(crate) next_up: Shelf,
    pub(crate) stack: Vec<Level>,
    pub(crate) query: String,
    pub(crate) results: Level,
    pub(crate) player: Option<PlayerStatus>,
    /// Until the first poll answers, "no daemon" is not yet a fact about it,
    /// so the footer must not report one.
    pub(crate) player_polled: bool,
    /// The playing item's duration, which the status socket does not carry.
    /// Keyed by item id so a stale total never labels a new episode.
    pub(crate) runtime_ticks: Option<(String, i64)>,
    /// The playing item and the rest of its season, both keyed by the item id
    /// they describe.
    pub(crate) playing_item: Option<(String, Item)>,
    pub(crate) playing_episodes: Level,
    /// Needed only to address commands, and `/Sessions` is expensive, so it is
    /// fetched once in the background rather than polled.
    session_id: Option<String>,
    pub(crate) covers: Covers,
    /// The last size the terminal reported, so a cover box can be worked out
    /// between frames rather than only while one is being drawn.
    viewport: Size,
    cover_due: Option<tokio::time::Instant>,
    wanted_covers: Vec<CoverKey>,
    paths: Paths,
    pub(crate) message: String,
    search_generation: u64,
    search_due: Option<tokio::time::Instant>,
    quit: bool,
}

impl App {
    pub(crate) fn new(api: Api, paths: Paths, picker: Picker) -> Self {
        let (tx, rx) = unbounded_channel();
        Self {
            api,
            tx,
            rx,
            screen: Screen::Home,
            home_pane: HomePane::Resume,
            resume: Shelf::default(),
            next_up: Shelf::default(),
            stack: Vec::new(),
            query: String::new(),
            results: Level::loading("Search", Source::Libraries),
            player: None,
            player_polled: false,
            runtime_ticks: None,
            playing_item: None,
            playing_episodes: Level::loading("Episodes", Source::Libraries),
            session_id: None,
            covers: Covers::new(picker),
            viewport: Size::default(),
            cover_due: None,
            wanted_covers: Vec::new(),
            paths,
            message: String::new(),
            search_generation: 0,
            search_due: None,
            quit: false,
        }
    }

    pub(crate) async fn run(mut self) -> Result<()> {
        let mut terminal = view::enter()?;
        let mut input = spawn_input_thread();
        let mut poll = tokio::time::interval(POLL_INTERVAL);
        poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        self.load_home();
        self.poll_player();
        self.load_session_id();

        let result = loop {
            if let Ok(viewport) = terminal.size() {
                self.viewport = viewport;
            }
            // Not an `Event::Resize` arm: a display of another scale can
            // change the cell's pixels without moving the grid at all.
            if let Ok(window) = terminal.backend_mut().window_size() {
                self.covers.set_cell_size(cover::cell_size(window));
            }
            self.tick_covers();
            if let Err(e) = terminal.draw(|frame| view::render(&self, frame)) {
                break Err(e.into());
            }
            let (search_deadline, cover_deadline) = (self.search_due, self.cover_due);
            tokio::select! {
                event = input.recv() => match event {
                    Some(event) => self.on_event(event),
                    None => break Ok(()),
                },
                Some(msg) = self.rx.recv() => self.on_msg(msg),
                _ = poll.tick() => self.poll_player(),
                _ = sleep_until(search_deadline) => {
                    self.search_due = None;
                    self.run_search();
                }
                _ = sleep_until(cover_deadline) => {
                    self.cover_due = None;
                    self.request_covers();
                }
            }
            if self.quit {
                break Ok(());
            }
        };
        view::leave();
        result
    }

    fn on_event(&mut self, event: Event) {
        let Event::Key(key) = event else { return };
        if key.kind == KeyEventKind::Release {
            return;
        }
        let typing = self.screen == Screen::Search;
        let Some(intent) = keys::map(key, typing) else {
            return;
        };
        self.apply(intent);
    }

    fn apply(&mut self, intent: Intent) {
        // Any keypress retires the previous message; handlers that want to
        // say something set it again below.
        self.message.clear();
        match intent {
            Intent::Quit => self.quit = true,
            Intent::Home => self.screen = Screen::Home,
            Intent::Libraries => self.open_libraries(),
            Intent::Playing => self.screen = Screen::Playing,
            Intent::StartSearch => {
                self.screen = Screen::Search;
                self.message.clear();
            }
            Intent::Up => self.move_vertically(-1),
            Intent::Down => self.move_vertically(1),
            Intent::PageUp => self.move_by(-self.page_step()),
            Intent::PageDown => self.move_by(self.page_step()),
            Intent::Left => self.move_in_grid(-1),
            Intent::Right => self.move_in_grid(1),
            Intent::Top => self.move_to_end(End::Top),
            Intent::Bottom => self.move_to_end(End::Bottom),
            Intent::NextPane => self.toggle_shelf(),
            Intent::Enter => self.enter(),
            Intent::Back => self.back(),
            Intent::Refresh => self.refresh(),
            Intent::Type(c) => {
                self.query.push(c);
                self.schedule_search();
            }
            Intent::Backspace => {
                self.query.pop();
                self.schedule_search();
            }
            Intent::PlayPause => self.send_playstate(PlaystateCommand::PlayPause, None),
            Intent::Stop => self.send_playstate(PlaystateCommand::Stop, None),
            Intent::Next => self.send_playstate(PlaystateCommand::NextTrack, None),
            Intent::Previous => self.send_playstate(PlaystateCommand::PreviousTrack, None),
            Intent::SeekBy(seconds) => self.seek_by(seconds),
            Intent::VolumeBy(delta) => self.volume_by(delta),
            Intent::ToggleMute => self.send_general("ToggleMute", json!({})),
            Intent::ToggleFullscreen => self.send_general("ToggleFullscreen", json!({})),
        }
    }
}

/// crossterm's `read` blocks, so it gets a thread of its own. The thread is
/// detached: it is parked in `read` at exit, and the process is leaving.
fn spawn_input_thread() -> UnboundedReceiver<Event> {
    let (tx, rx) = unbounded_channel();
    std::thread::spawn(move || {
        while let Ok(event) = ratatui::crossterm::event::read() {
            if tx.send(event).is_err() {
                break;
            }
        }
    });
    rx
}

/// A `select!` arm that is simply never ready when nothing is pending.
async fn sleep_until(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
#[path = "mod_test.rs"]
mod tests;
