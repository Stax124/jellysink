//! App state and the event loop.

use super::cover::{self, CoverKey, Covers};
use super::grid;
use super::keys::{self, Intent};
use super::nav::{self, End, Level, Source};
use super::playing;
use super::rail;
use super::ui;
use color_eyre::eyre::Result;
use jellysink_core::config::Paths;
use jellysink_core::instance;
use jellysink_core::jellyfin::auth::Api;
use jellysink_core::jellyfin::browse::{EPISODE_LIMIT, ItemQuery};
use jellysink_core::jellyfin::model::{Item, ItemList};
use jellysink_core::jellyfin::remote::PlaystateCommand;
use jellysink_core::status::PlayerStatus;
use jellysink_core::ticks::seconds_to_ticks;
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
pub(super) enum Screen {
    Home,
    Browse,
    Search,
    Playing,
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
    /// Both carry the item id they were asked for, so a reply that arrives
    /// after playback moved on is dropped rather than describing the wrong
    /// episode — the rule `specs/tui.md` already sets for search and levels.
    PlayingItem {
        item_id: String,
        item: Box<Item>,
    },
    PlayingEpisodes {
        item_id: String,
        items: Vec<Item>,
    },
    /// `None` when the server has no artwork for the item — quiet and common.
    Cover {
        key: CoverKey,
        protocol: Option<Box<Protocol>>,
    },
    /// The request failed rather than answered, so the item keeps its claim to
    /// a cover and is asked for again next time it is on screen.
    CoverFailed {
        key: CoverKey,
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
    pub(super) home_offset: usize,
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
    /// The playing item and the rest of its season, both keyed by the item id
    /// they describe.
    pub(super) playing_item: Option<(String, Item)>,
    pub(super) playing_episodes: Level,
    /// Needed only to address commands, and `/Sessions` is expensive, so it is
    /// fetched once in the background rather than polled.
    session_id: Option<String>,
    pub(super) covers: Covers,
    /// The last size the terminal reported, so a cover box can be worked out
    /// between frames rather than only while one is being drawn.
    viewport: Size,
    cover_due: Option<tokio::time::Instant>,
    wanted_covers: Vec<CoverKey>,
    paths: Paths,
    pub(super) message: String,
    search_generation: u64,
    search_due: Option<tokio::time::Instant>,
    quit: bool,
}

impl App {
    pub(super) fn new(api: Api, paths: Paths, picker: Picker, image_scale: f32) -> Self {
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
            home_offset: 0,
            stack: Vec::new(),
            query: String::new(),
            results: Level::loading("Search", Source::Libraries),
            player: None,
            player_polled: false,
            runtime_ticks: None,
            playing_item: None,
            playing_episodes: Level::loading("Episodes", Source::Libraries),
            session_id: None,
            covers: Covers::new(picker, image_scale),
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

    pub(super) async fn run(mut self) -> Result<()> {
        let mut terminal = ui::enter()?;
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
            self.tick_covers();
            if let Err(e) = terminal.draw(|frame| ui::render(&self, frame)) {
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
            Intent::Playing => self.screen = Screen::Playing,
            Intent::StartSearch => {
                self.screen = Screen::Search;
                self.message.clear();
            }
            Intent::Up => self.move_by(-self.row_step()),
            Intent::Down => self.move_by(self.row_step()),
            Intent::PageUp => self.move_by(-self.page_step()),
            Intent::PageDown => self.move_by(self.page_step()),
            Intent::Left => self.move_in_grid(-1),
            Intent::Right => self.move_in_grid(1),
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
            Msg::PlayingItem { item_id, item } => {
                if self.is_current(&item_id) {
                    if let Some((series_id, season_id)) =
                        item.series_id.clone().zip(item.season_id.clone())
                    {
                        self.load_playing_episodes(item_id.clone(), series_id, season_id);
                    }
                    self.playing_item = Some((item_id, *item));
                }
            }
            Msg::PlayingEpisodes { item_id, items } => {
                if self.is_current(&item_id) {
                    self.playing_episodes.fill(items);
                    if let Some(index) = self
                        .playing_episodes
                        .items
                        .iter()
                        .position(|episode| episode.id == item_id)
                    {
                        self.playing_episodes.selected = index;
                    }
                }
            }
            Msg::Cover { key, protocol } => {
                self.covers.store(key, protocol.map(|boxed| *boxed));
            }
            Msg::CoverFailed { key } => self.covers.release(&key),
            Msg::Error(message) => self.message = message,
        }
    }

    fn rows(&self) -> &[Item] {
        match self.screen {
            Screen::Home => self.home_rows(),
            Screen::Browse => self.stack.last().map_or(&[], |level| &level.items),
            Screen::Search => &self.results.items,
            Screen::Playing => &self.playing_episodes.items,
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
            Screen::Playing => self.playing_episodes.selected,
        }
    }

    /// The looked-up item, only while it still describes what is playing.
    pub(super) fn current_item(&self) -> Option<&Item> {
        let now_playing = self.now_playing()?;
        self.playing_item
            .as_ref()
            .filter(|(item_id, _)| *item_id == now_playing.item_id)
            .map(|(_, item)| item)
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
            Screen::Playing => self.playing_episodes.move_by(delta),
        }
        self.rescroll();
    }

    /// Only a grid has a second axis. A list ignores these rather than making
    /// the arrows a second, unadvertised way to do Esc and Enter.
    fn move_in_grid(&mut self, delta: isize) {
        if self.grid_metrics().is_some() {
            self.move_by(delta);
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
            Screen::Playing => self.playing_episodes.move_to_end(end),
        }
        self.rescroll();
    }

    /// How far one press of up or down travels: a whole row in a grid.
    fn row_step(&self) -> isize {
        self.grid_metrics()
            .and_then(|metrics| isize::try_from(metrics.columns).ok())
            .unwrap_or(1)
    }

    fn page_step(&self) -> isize {
        self.grid_metrics()
            .and_then(|metrics| isize::try_from(metrics.page()).ok())
            .unwrap_or(PAGE_JUMP)
    }

    /// The grid the focused screen is drawing, if it is drawing one. Search
    /// stays a list whatever it turned up, because its rows are mixed kinds.
    pub(super) fn grid_metrics(&self) -> Option<grid::Metrics> {
        let shows_grid = match self.screen {
            Screen::Home => true,
            Screen::Browse => nav::is_grid(self.rows()),
            // Search rows are mixed kinds, and the Playing screen draws one
            // still of its own.
            Screen::Search | Screen::Playing => false,
        };
        let first = self.rows().first()?;
        shows_grid.then(|| {
            grid::metrics(
                grid::inner(self.body_area()),
                cover::primary_aspect(first),
                self.covers.font_size(),
            )
        })
    }

    fn body_area(&self) -> Rect {
        ui::panes(Rect::new(0, 0, self.viewport.width, self.viewport.height)).body
    }

    pub(super) fn grid_offset(&self) -> usize {
        match self.screen {
            Screen::Home => self.home_offset,
            Screen::Browse => self.stack.last().map_or(0, |level| level.offset),
            Screen::Search | Screen::Playing => 0,
        }
    }

    /// Scrolls the grid the least that brings the cursor back on screen.
    fn rescroll(&mut self) {
        let Some(metrics) = self.grid_metrics() else {
            return;
        };
        let offset = grid::scroll_to(self.grid_offset(), self.selected(), &metrics);
        match self.screen {
            Screen::Home => self.home_offset = offset,
            Screen::Browse => {
                if let Some(level) = self.stack.last_mut() {
                    level.offset = offset;
                }
            }
            Screen::Search | Screen::Playing => {}
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
        self.home_offset = 0;
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
            Screen::Playing => self.screen = Screen::Home,
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
            Screen::Playing => {
                if let Some(item_id) = self.now_playing().map(|np| np.item_id.clone()) {
                    self.load_playing(item_id);
                }
            }
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

    /// The item whose art the rail is showing. Home has no rail.
    fn rail_item(&self) -> Option<&Item> {
        match self.screen {
            Screen::Browse | Screen::Search => self.selected_item(),
            Screen::Home | Screen::Playing => None,
        }
    }

    /// Every cover the current screen wants. Recomputed each iteration, which
    /// is what makes a terminal resize ask for the new size without anything
    /// having to notice the resize itself.
    fn visible_covers(&self) -> Vec<CoverKey> {
        if self.screen == Screen::Playing {
            let (body, font_size) = (self.body_area(), self.covers.font_size());
            return self
                .current_item()
                .and_then(|item| playing::still_size(body, item, font_size).zip(Some(item)))
                .and_then(|(size, item)| CoverKey::primary(item, size))
                .into_iter()
                .collect();
        }
        if let Some(metrics) = self.grid_metrics() {
            let size = metrics.cover_size();
            return self
                .rows()
                .iter()
                .skip(self.grid_offset() * metrics.columns)
                .take(metrics.page())
                .filter_map(|item| CoverKey::primary(item, size))
                .collect();
        }
        let (body, font_size) = (self.body_area(), self.covers.font_size());
        self.rail_item()
            .and_then(|item| rail::cover_size(body, item, font_size).zip(Some(item)))
            .and_then(|(size, item)| CoverKey::primary(item, size))
            .into_iter()
            .collect()
    }

    fn tick_covers(&mut self) {
        let wanted = self.visible_covers();
        if wanted != self.wanted_covers {
            self.wanted_covers = wanted;
            self.cover_due = Some(tokio::time::Instant::now() + COVER_DEBOUNCE);
        }
    }

    /// The trailing edge of the debounce. `Covers::claim` is what keeps a
    /// resting cursor, and a second visit to the same row, to one request.
    fn request_covers(&mut self) {
        for key in self.wanted_covers.clone() {
            if !self.covers.claim(&key) {
                continue;
            }
            let (api, tx) = (self.api.clone(), self.tx.clone());
            let (picker, scale) = (self.covers.picker(), self.covers.scale());
            tokio::spawn(async move {
                let msg = match cover::fetch(&api, picker, scale, &key).await {
                    Ok(protocol) => Msg::Cover {
                        key,
                        protocol: protocol.map(Box::new),
                    },
                    Err(_) => Msg::CoverFailed { key },
                };
                let _ = tx.send(msg);
            });
        }
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
            self.playing_item = None;
            return;
        };
        if self
            .runtime_ticks
            .as_ref()
            .is_none_or(|(id, _)| *id != item_id)
        {
            self.load_runtime_ticks(item_id.clone());
            self.load_playing(item_id);
        }
    }

    fn is_current(&self, item_id: &str) -> bool {
        self.now_playing()
            .is_some_and(|now_playing| now_playing.item_id == item_id)
    }

    /// The Playing screen's own lookup: the status socket carries a title and
    /// a position, not a synopsis or a season.
    fn load_playing(&mut self, item_id: String) {
        self.playing_item = None;
        self.playing_episodes = Level::loading("Episodes", Source::Libraries);
        let (api, tx) = (self.api.clone(), self.tx.clone());
        tokio::spawn(async move {
            let msg = match api.get_item(&item_id).await {
                Ok(value) => match Item::deserialize(&value) {
                    Ok(item) => Msg::PlayingItem {
                        item_id,
                        item: Box::new(item),
                    },
                    Err(e) => Msg::Error(format!("decoding item: {e}")),
                },
                Err(e) => Msg::Error(format!("{e:#}")),
            };
            let _ = tx.send(msg);
        });
    }

    fn load_playing_episodes(&self, item_id: String, series_id: String, season_id: String) {
        let api = self.api.clone();
        self.spawn(
            async move {
                api.episodes(&series_id, Some(&season_id), EPISODE_LIMIT)
                    .await
            },
            move |items| Msg::PlayingEpisodes { item_id, items },
        );
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
        // These share the header with the tabs, so they have to stay short
        // enough to survive `ui::to_width` on an 80-column terminal.
        self.message = if self.player.is_some() {
            // Connected, but the one-off lookup has not landed yet.
            self.load_session_id();
            "looking up the session — try again".to_string()
        } else {
            "jellysink not connected".to_string()
        };
        None
    }

    fn play(&mut self, item: &Item) {
        let Some(session_id) = self.session_id() else {
            return;
        };
        let (api, tx) = (self.api.clone(), self.tx.clone());
        let (item_id, start_ticks) = (item.id.clone(), item.resume_ticks());
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
    pub(super) fn now_playing(&self) -> Option<&jellysink_core::status::NowPlaying> {
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
