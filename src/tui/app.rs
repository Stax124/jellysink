//! App state and the event loop.

use super::keys::{self, Intent};
use super::nav::{self, End, Level, Source};
use super::ui;
use crate::app::config::Paths;
use crate::app::instance;
use crate::jellyfin::auth::Api;
use crate::jellyfin::browse::{EPISODE_LIMIT, ItemQuery};
use crate::jellyfin::model::{Item, ItemList};
use crate::jellyfin::remote::PlaystateCommand;
use crate::runtime::PlayerStatus;
use crate::ticks::seconds_to_ticks;
use color_eyre::eyre::Result;
use ratatui::crossterm::event::{Event, KeyEventKind};
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
const SEARCH_TYPES: &str = "Movie,Series,Episode";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Screen {
    Home,
    Browse,
    Search,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HomePane {
    Resume,
    NextUp,
}

/// Results arriving from the spawned request tasks.
enum Msg {
    Home(HomePane, Vec<Item>),
    Level(usize, Vec<Item>),
    Search(u64, Vec<Item>),
    /// `None` when the daemon did not answer, which the footer reports as
    /// "not connected" — the only failure this socket really has.
    Player(Option<Box<PlayerStatus>>),
    SessionId(String),
    Runtime {
        item_id: String,
        ticks: i64,
    },
    Error(String),
}

pub(super) struct App {
    api: Api,
    tx: UnboundedSender<Msg>,
    rx: UnboundedReceiver<Msg>,
    pub(super) screen: Screen,
    pub(super) home_pane: HomePane,
    pub(super) resume: Vec<Item>,
    pub(super) next_up: Vec<Item>,
    pub(super) home_selected: usize,
    pub(super) stack: Vec<Level>,
    pub(super) query: String,
    pub(super) results: Level,
    pub(super) player: Option<PlayerStatus>,
    /// Until the first poll answers, "no daemon" is not yet a fact about it,
    /// so the footer must not report one.
    pub(super) player_polled: bool,
    /// The playing item's duration, which the status socket does not carry.
    /// Keyed by item id so a stale total never labels a new episode.
    pub(super) runtime_ticks: Option<(String, i64)>,
    /// Needed only to address commands, and `/Sessions` is expensive, so it is
    /// fetched once in the background rather than polled.
    session_id: Option<String>,
    paths: Paths,
    pub(super) message: String,
    search_generation: u64,
    search_due: Option<tokio::time::Instant>,
    quit: bool,
}

impl App {
    pub(super) fn new(api: Api, paths: Paths) -> Self {
        let (tx, rx) = unbounded_channel();
        Self {
            api,
            tx,
            rx,
            screen: Screen::Home,
            home_pane: HomePane::Resume,
            resume: Vec::new(),
            next_up: Vec::new(),
            home_selected: 0,
            stack: Vec::new(),
            query: String::new(),
            results: Level::loading("Search", Source::Libraries),
            player: None,
            player_polled: false,
            runtime_ticks: None,
            session_id: None,
            paths,
            message: String::new(),
            search_generation: 0,
            search_due: None,
            quit: false,
        }
    }

    pub(super) async fn run(mut self) -> Result<()> {
        let mut terminal = ui::enter()?;
        let mut input = spawn_input_thread();
        let mut poll = tokio::time::interval(POLL_INTERVAL);
        poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        self.load_home();
        self.poll_player();
        self.load_session_id();

        let result = loop {
            if let Err(e) = terminal.draw(|frame| ui::render(&self, frame)) {
                break Err(e.into());
            }
            let search_deadline = self.search_due;
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
            }
            if self.quit {
                break Ok(());
            }
        };
        ui::leave();
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
            Intent::StartSearch => {
                self.screen = Screen::Search;
                self.message.clear();
            }
            Intent::Up => self.move_by(-1),
            Intent::Down => self.move_by(1),
            Intent::PageUp => self.move_by(-PAGE_JUMP),
            Intent::PageDown => self.move_by(PAGE_JUMP),
            Intent::Top => self.move_to_end(End::Top),
            Intent::Bottom => self.move_to_end(End::Bottom),
            Intent::NextPane => self.toggle_home_pane(),
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

    fn on_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Home(HomePane::Resume, items) => self.resume = items,
            Msg::Home(HomePane::NextUp, items) => self.next_up = items,
            Msg::Level(depth, items) => {
                if let Some(level) = self.stack.get_mut(depth) {
                    level.fill(items);
                }
            }
            // A slower earlier request must not overwrite a newer result.
            Msg::Search(generation, items) => {
                if generation == self.search_generation {
                    self.results.fill(items);
                }
            }
            Msg::Player(player) => self.on_player(player.map(|boxed| *boxed)),
            Msg::SessionId(session_id) => self.session_id = Some(session_id),
            Msg::Runtime { item_id, ticks } => self.runtime_ticks = Some((item_id, ticks)),
            Msg::Error(message) => self.message = message,
        }
    }

    fn rows(&self) -> &[Item] {
        match self.screen {
            Screen::Home => self.home_rows(),
            Screen::Browse => self.stack.last().map_or(&[], |level| &level.items),
            Screen::Search => &self.results.items,
        }
    }

    pub(super) fn home_rows(&self) -> &[Item] {
        match self.home_pane {
            HomePane::Resume => &self.resume,
            HomePane::NextUp => &self.next_up,
        }
    }

    pub(super) fn selected(&self) -> usize {
        match self.screen {
            Screen::Home => self.home_selected,
            Screen::Browse => self.stack.last().map_or(0, |level| level.selected),
            Screen::Search => self.results.selected,
        }
    }

    fn selected_item(&self) -> Option<&Item> {
        self.rows().get(self.selected())
    }

    fn move_by(&mut self, delta: isize) {
        match self.screen {
            Screen::Home => {
                let len = self.home_rows().len();
                self.home_selected = if len == 0 {
                    0
                } else {
                    self.home_selected.saturating_add_signed(delta).min(len - 1)
                };
            }
            Screen::Browse => {
                if let Some(level) = self.stack.last_mut() {
                    level.move_by(delta);
                }
            }
            Screen::Search => self.results.move_by(delta),
        }
    }

    fn move_to_end(&mut self, end: End) {
        match self.screen {
            Screen::Home => {
                self.home_selected = match end {
                    End::Top => 0,
                    End::Bottom => self.home_rows().len().saturating_sub(1),
                }
            }
            Screen::Browse => {
                if let Some(level) = self.stack.last_mut() {
                    level.move_to_end(end);
                }
            }
            Screen::Search => self.results.move_to_end(end),
        }
    }

    fn toggle_home_pane(&mut self) {
        if self.screen != Screen::Home {
            return;
        }
        self.home_pane = match self.home_pane {
            HomePane::Resume => HomePane::NextUp,
            HomePane::NextUp => HomePane::Resume,
        };
        self.home_selected = 0;
    }

    fn enter(&mut self) {
        let Some(item) = self.selected_item().cloned() else {
            return;
        };
        match nav::descend(&item) {
            Some(source) => {
                self.screen = Screen::Browse;
                self.push(item.name.clone().unwrap_or_default(), source);
            }
            None => self.play(&item),
        }
    }

    fn back(&mut self) {
        match self.screen {
            Screen::Search => {
                self.screen = Screen::Home;
                self.query.clear();
            }
            Screen::Browse => {
                self.stack.pop();
                if self.stack.is_empty() {
                    self.screen = Screen::Home;
                }
            }
            Screen::Home => {}
        }
    }

    fn open_libraries(&mut self) {
        self.screen = Screen::Browse;
        self.stack.clear();
        self.push("Libraries", Source::Libraries);
    }

    fn push(&mut self, title: impl Into<String>, source: Source) {
        self.stack.push(Level::loading(title, source.clone()));
        self.load_level(self.stack.len() - 1, source);
    }

    fn refresh(&mut self) {
        match self.screen {
            Screen::Home => self.load_home(),
            Screen::Browse => {
                if let Some((depth, source)) = self
                    .stack
                    .last()
                    .map(|level| (self.stack.len() - 1, level.source.clone()))
                {
                    self.load_level(depth, source);
                }
            }
            Screen::Search => self.run_search(),
        }
        self.poll_player();
    }

    /// Every load goes through here, so no request can block key handling.
    fn spawn<F>(&self, request: F, wrap: impl FnOnce(Vec<Item>) -> Msg + Send + 'static)
    where
        F: std::future::Future<Output = Result<serde_json::Value>> + Send + 'static,
    {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let msg = match request.await {
                Ok(body) => match ItemList::deserialize(&body) {
                    Ok(list) => wrap(list.items),
                    Err(e) => Msg::Error(format!("decoding items: {e}")),
                },
                Err(e) => Msg::Error(format!("{e:#}")),
            };
            let _ = tx.send(msg);
        });
    }

    fn load_home(&mut self) {
        let (api, limit) = (self.api.clone(), HOME_ROWS);
        self.spawn(async move { api.resume(limit).await }, |items| {
            Msg::Home(HomePane::Resume, items)
        });
        let api = self.api.clone();
        self.spawn(async move { api.next_up(limit).await }, |items| {
            Msg::Home(HomePane::NextUp, items)
        });
    }

    fn load_level(&self, depth: usize, source: Source) {
        let api = self.api.clone();
        self.spawn(
            async move {
                match source {
                    Source::Libraries => api.user_views().await,
                    Source::Folder(parent_id) => api.items(&ItemQuery::in_folder(&parent_id)).await,
                    Source::Seasons(series_id) => api.seasons(&series_id).await,
                    Source::Episodes {
                        series_id,
                        season_id,
                    } => {
                        api.episodes(&series_id, Some(&season_id), EPISODE_LIMIT)
                            .await
                    }
                }
            },
            move |items| Msg::Level(depth, items),
        );
    }

    fn schedule_search(&mut self) {
        self.search_due = Some(tokio::time::Instant::now() + SEARCH_DEBOUNCE);
    }

    fn run_search(&mut self) {
        self.search_generation += 1;
        let generation = self.search_generation;
        if self.query.trim().is_empty() {
            self.results.fill(Vec::new());
            return;
        }
        self.results.loading = true;
        let (api, term) = (self.api.clone(), self.query.clone());
        self.spawn(
            async move {
                api.items(
                    &ItemQuery::search(&term)
                        .with_types(SEARCH_TYPES)
                        .page(0, SEARCH_LIMIT),
                )
                .await
            },
            move |items| Msg::Search(generation, items),
        );
    }

    /// Footer state comes from the daemon's own status socket, not from
    /// `GET /Sessions`: that response embeds `NowPlayingQueueFullItems` and
    /// runs to megabytes once a series is queued, which cannot be trimmed by
    /// any request parameter. This is a few dozen bytes over a Unix socket.
    fn poll_player(&self) {
        let (paths, tx) = (self.paths.clone(), self.tx.clone());
        tokio::spawn(async move {
            let status = tokio::task::spawn_blocking(move || instance::request_status(&paths))
                .await
                .ok()
                .and_then(Result::ok);
            let _ = tx.send(Msg::Player(status.map(Box::new)));
        });
    }

    /// Once per process: see [`Msg::SessionId`].
    fn load_session_id(&self) {
        let (api, tx) = (self.api.clone(), self.tx.clone());
        tokio::spawn(async move {
            let msg = match api.session_for_device().await {
                Ok(Some(session)) => Msg::SessionId(session.id),
                Ok(None) => return,
                Err(e) => Msg::Error(format!("{e:#}")),
            };
            let _ = tx.send(msg);
        });
    }

    fn on_player(&mut self, player: Option<PlayerStatus>) {
        self.player_polled = true;
        let item_id = player
            .as_ref()
            .and_then(|status| status.now_playing.as_ref())
            .map(|now_playing| now_playing.item_id.clone());
        self.player = player;
        let Some(item_id) = item_id else {
            self.runtime_ticks = None;
            return;
        };
        if self
            .runtime_ticks
            .as_ref()
            .is_none_or(|(id, _)| *id != item_id)
        {
            self.load_runtime_ticks(item_id);
        }
    }

    /// One small request per item change, rather than a duration in every
    /// status poll.
    fn load_runtime_ticks(&self, item_id: String) {
        let (api, tx) = (self.api.clone(), self.tx.clone());
        tokio::spawn(async move {
            let msg = match api.get_item(&item_id).await {
                Ok(value) => match value
                    .get("RunTimeTicks")
                    .and_then(serde_json::Value::as_i64)
                {
                    Some(ticks) => Msg::Runtime { item_id, ticks },
                    None => Msg::Error(format!("item {item_id} has no RunTimeTicks")),
                },
                Err(e) => Msg::Error(format!("{e:#}")),
            };
            let _ = tx.send(msg);
        });
    }

    fn session_id(&mut self) -> Option<String> {
        if let Some(session_id) = &self.session_id {
            return Some(session_id.clone());
        }
        self.message = if self.player.is_some() {
            // Connected, but the one-off lookup has not landed yet.
            self.load_session_id();
            "still looking up the jellysink session — try again".to_string()
        } else {
            "jellysink is not connected — start it with `systemctl --user start jellysink`"
                .to_string()
        };
        None
    }

    fn play(&mut self, item: &Item) {
        let Some(session_id) = self.session_id() else {
            return;
        };
        let (api, tx) = (self.api.clone(), self.tx.clone());
        let (item_id, start_ticks) = (item.id.clone(), item.resume_ticks());
        self.message = format!("Playing {}", item.label());
        tokio::spawn(async move {
            if let Err(e) = api.play_now(&session_id, &item_id, start_ticks).await {
                let _ = tx.send(Msg::Error(format!("{e:#}")));
            }
        });
    }

    fn send_playstate(&mut self, command: PlaystateCommand, seek_ticks: Option<i64>) {
        let Some(session_id) = self.session_id() else {
            return;
        };
        let (api, tx) = (self.api.clone(), self.tx.clone());
        tokio::spawn(async move {
            if let Err(e) = api.playstate(&session_id, command, seek_ticks).await {
                let _ = tx.send(Msg::Error(format!("{e:#}")));
            }
        });
    }

    fn send_general(&mut self, name: &'static str, arguments: serde_json::Value) {
        let Some(session_id) = self.session_id() else {
            return;
        };
        let (api, tx) = (self.api.clone(), self.tx.clone());
        tokio::spawn(async move {
            if let Err(e) = api.general_command(&session_id, name, arguments).await {
                let _ = tx.send(Msg::Error(format!("{e:#}")));
            }
        });
    }

    fn seek_by(&mut self, seconds: i64) {
        let Some(position) = self.position_ticks() else {
            return;
        };
        self.send_playstate(PlaystateCommand::Seek, Some(seek_target(position, seconds)));
    }

    fn volume_by(&mut self, delta: i64) {
        let current = self
            .player
            .as_ref()
            .and_then(|status| status.now_playing.as_ref())
            .map_or(100, |now_playing| now_playing.volume);
        let volume = (current + delta).clamp(0, 100);
        self.send_general("SetVolume", json!({ "Volume": volume.to_string() }));
    }
}

impl App {
    pub(super) fn now_playing(&self) -> Option<&crate::runtime::status::NowPlaying> {
        self.player.as_ref()?.now_playing.as_ref()
    }

    pub(super) fn position_ticks(&self) -> Option<i64> {
        self.now_playing()
            .map(|now_playing| now_playing.position_ticks)
    }

    /// The current item's duration, only if it belongs to the current item.
    pub(super) fn total_ticks(&self) -> Option<i64> {
        let now_playing = self.now_playing()?;
        self.runtime_ticks
            .as_ref()
            .filter(|(item_id, _)| *item_id == now_playing.item_id)
            .map(|(_, ticks)| *ticks)
    }
}

/// Seeking is absolute over the wire, so the target is computed from the last
/// polled position — up to a second stale, which at ten-second steps is not
/// noticeable.
fn seek_target(position_ticks: i64, seconds: i64) -> i64 {
    (position_ticks + seconds_to_ticks(seconds as f64)).max(0)
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
#[path = "app_test.rs"]
mod tests;
