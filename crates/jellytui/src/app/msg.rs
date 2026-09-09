//! What the spawned request tasks send back, and where it lands.

use super::*;

/// Results arriving from the spawned request tasks.
pub(super) enum Msg {
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

impl App {
    pub(super) fn on_msg(&mut self, msg: Msg) {
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

    /// Every load goes through here, so no request can block key handling.
    pub(super) fn spawn<F>(&self, request: F, wrap: impl FnOnce(Vec<Item>) -> Msg + Send + 'static)
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
}
