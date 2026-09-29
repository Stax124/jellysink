//! What the spawned request tasks send back, and where it lands.

use super::*;

pub(super) enum Msg {
    Home(HomePane, Vec<Item>),
    /// Lands only on the level at that depth that asked for that source.
    Level(usize, Source, Vec<Item>),
    Search(u64, Vec<Item>),
    /// `None` when the daemon did not answer, which the footer reports as
    /// "not connected" — the only failure this socket really has.
    Player(Option<Box<PlayerStatus>>),
    SessionId(String),
    /// Both carry the item id they were asked for, so a reply arriving after
    /// playback moved on is dropped rather than describing the wrong episode.
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
    CoverFailed(CoverKey),
    /// Carries no item id: it triggers a refetch rather than writing into a
    /// row, so there is nothing an id could keep it from landing on.
    Watched,
    UpdateChecked(UpdateCheck),
    Error(String),
}

impl From<color_eyre::Report> for Msg {
    fn from(e: color_eyre::Report) -> Self {
        Self::Error(format!("{e:#}"))
    }
}

impl App {
    pub(super) fn on_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Home(pane, items) => self.shelf_mut(pane).fill(items),
            Msg::Level(depth, source, items) => {
                if let Some(level) = self.stack.get_mut(depth)
                    && level.source == source
                {
                    level.rows.fill(items);
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
            Msg::CoverFailed(key) => self.covers.give_up(&key),
            Msg::Watched => self.reload_screen_and_home(),
            Msg::UpdateChecked(update) => self.update = update,
            Msg::Error(message) => {
                tracing::warn!(%message, "request failed");
                self.message = message;
            }
        }
    }

    /// Runs `task` off the loop, so no request can block key handling, and
    /// delivers what it returns.
    pub(super) fn spawn_msg(&self, task: impl Future<Output = Option<Msg>> + Send + 'static) {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            if let Some(msg) = task.await {
                // The receiver is gone only once the loop has ended.
                let _ = tx.send(msg);
            }
        });
    }

    /// A listing request; `label` names it in the log pane and in the timing.
    pub(super) fn spawn<F>(
        &self,
        label: &'static str,
        request: F,
        wrap: impl FnOnce(Vec<Item>) -> Msg + Send + 'static,
    ) where
        F: Future<Output = Result<serde_json::Value>> + Send + 'static,
    {
        self.spawn_msg(async move {
            let started = std::time::Instant::now();
            let list = request
                .await
                .and_then(|body| ItemList::deserialize(&body).wrap_err("decoding items"));
            Some(match list {
                Ok(list) => {
                    tracing::info!(
                        label,
                        rows = list.items.len(),
                        elapsed_ms = started.elapsed().as_millis(),
                        "loaded"
                    );
                    wrap(list.items)
                }
                Err(e) => e.into(),
            })
        });
    }
}
