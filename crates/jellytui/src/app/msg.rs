//! What the spawned request tasks send back, and where it lands.

use super::*;

pub(super) enum Msg {
    Home(HomePane, Vec<Item>),
    Level(usize, Vec<Item>),
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
    /// The request failed rather than answered. It carries the error because
    /// the tile stays blank either way, which is no evidence of a failure.
    CoverFailed {
        key: CoverKey,
        error: String,
    },
    /// Carries no item id: it triggers a refetch rather than writing into a
    /// row, so there is nothing an id could keep it from landing on.
    Watched,
    UpdateAvailable(String),
    Error(String),
}

impl App {
    pub(super) fn on_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Home(pane, items) => self.shelf_mut(pane).fill(items),
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
            Msg::CoverFailed { key, error } => {
                tracing::debug!(%error, "cover request failed");
                self.covers.give_up(&key);
            }
            Msg::Watched => self.reload_screen_and_home(),
            Msg::UpdateAvailable(version) => self.update_offer = Some(version),
            Msg::Error(message) => {
                tracing::warn!(%message, "request failed");
                self.message = message;
            }
        }
    }

    /// Every load goes through here, so no request can block key handling.
    /// `label` names it in the log pane and in the timing.
    pub(super) fn spawn<F>(
        &self,
        label: &'static str,
        request: F,
        wrap: impl FnOnce(Vec<Item>) -> Msg + Send + 'static,
    ) where
        F: std::future::Future<Output = Result<serde_json::Value>> + Send + 'static,
    {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let started = std::time::Instant::now();
            let msg = match request.await {
                Ok(body) => match ItemList::deserialize(&body) {
                    Ok(list) => {
                        tracing::info!(
                            label,
                            rows = list.items.len(),
                            elapsed_ms = started.elapsed().as_millis(),
                            "loaded"
                        );
                        wrap(list.items)
                    }
                    Err(e) => Msg::Error(format!("decoding items: {e}")),
                },
                Err(e) => Msg::Error(format!("{e:#}")),
            };
            let _ = tx.send(msg);
        });
    }
}
