//! Every outbound browse request. Each spawns a task so HTTP never blocks a
//! keystroke; the answers come back as a [`Msg`].

use super::*;

impl App {
    pub(super) fn load_home(&mut self) {
        let (api, limit) = (self.api.clone(), HOME_ROWS);
        self.spawn("resume", async move { api.resume(limit).await }, |items| {
            Msg::Home(HomePane::Resume, items)
        });
        let api = self.api.clone();
        self.spawn(
            "next_up",
            async move { api.next_up(limit).await },
            |items| Msg::Home(HomePane::NextUp, items),
        );
    }

    pub(super) fn load_level(&self, depth: usize, source: Source) {
        tracing::info!(depth, ?source, "opening level");
        let api = self.api.clone();
        self.spawn(
            "level",
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

    /// Every cover the current screen wants. Recomputed each iteration, which
    /// is what makes a terminal resize ask for the new size without anything
    /// having to notice the resize itself.
    pub(super) fn visible_covers(&self) -> Vec<CoverKey> {
        if self.screen == Screen::Playing {
            let (body, font_size) = (self.body_area(), self.covers.font_size());
            return self
                .current_item()
                .and_then(|item| playing::still_size(body, item, font_size).zip(Some(item)))
                .and_then(|(size, item)| self.covers.key(item, size))
                .into_iter()
                .collect();
        }
        // Home draws both shelves, so the unfocused one's covers are on
        // screen too — `grid_metrics` only knows about the focused one.
        if self.screen == Screen::Home {
            let mut keys = Vec::new();
            for pane in HomePane::ALL {
                let Some(metrics) = self.shelf_metrics(pane) else {
                    continue;
                };
                let shelf = self.shelf(pane);
                keys.extend(
                    shelf
                        .items
                        .iter()
                        .skip(shelf.offset * metrics.columns)
                        .take(metrics.page())
                        .filter_map(|item| self.covers.key(item, metrics.cover_size())),
                );
            }
            return keys;
        }
        if let Some(metrics) = self.grid_metrics() {
            let size = metrics.cover_size();
            return self
                .rows()
                .iter()
                .skip(self.grid_offset() * metrics.columns)
                .take(metrics.page())
                .filter_map(|item| self.covers.key(item, size))
                .collect();
        }
        let (body, font_size) = (self.body_area(), self.covers.font_size());
        self.rail_item()
            .and_then(|item| rail::cover_size(body, item, font_size).zip(Some(item)))
            .and_then(|(size, item)| self.covers.key(item, size))
            .into_iter()
            .collect()
    }

    pub(super) fn tick_covers(&mut self) {
        let wanted = self.visible_covers();
        if wanted == self.wanted_covers {
            return;
        }
        self.wanted_covers = wanted;
        let now = tokio::time::Instant::now();
        match self.cover_ready_at {
            // Inside the window: wait it out rather than asking for a row the
            // cursor is only passing through. The last change to arrive is the
            // one still standing when it opens, so a cursor coming to rest is
            // always fetched.
            Some(ready_at) if ready_at > now => self.cover_due = Some(ready_at),
            _ => {
                self.cover_due = None;
                self.request_covers();
            }
        }
    }

    /// `Covers::claim` is what keeps a resting cursor, and a second visit to
    /// the same row, to one request.
    pub(super) fn request_covers(&mut self) {
        let mut spawned = 0usize;
        for key in self.wanted_covers.clone() {
            if !self.covers.claim(&key) {
                continue;
            }
            spawned += 1;
            let (api, tx) = (self.api.clone(), self.tx.clone());
            let picker = self.covers.picker();
            tokio::spawn(async move {
                let msg = match cover::fetch(&api, picker, &key).await {
                    Ok(protocol) => Msg::Cover {
                        key,
                        protocol: protocol.map(Box::new),
                    },
                    Err(_) => Msg::CoverFailed { key },
                };
                let _ = tx.send(msg);
            });
        }
        // A batch that asked for nothing — every cover already cached or in
        // flight — costs the server nothing, so it does not close the window.
        if spawned > 0 {
            self.cover_ready_at = Some(tokio::time::Instant::now() + COVER_THROTTLE);
        }
    }

    pub(super) fn schedule_search(&mut self) {
        self.search_due = Some(tokio::time::Instant::now() + SEARCH_DEBOUNCE);
    }

    pub(super) fn run_search(&mut self) {
        self.search_generation += 1;
        let generation = self.search_generation;
        if self.query.trim().is_empty() {
            self.results.fill(Vec::new());
            return;
        }
        self.results.loading = true;
        let (api, term) = (self.api.clone(), self.query.clone());
        tracing::info!(term = %self.query, generation, "searching");
        self.spawn(
            "search",
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

    /// The Playing screen's own lookup: the status socket carries a title and
    /// a position, not a synopsis or a season.
    pub(super) fn load_playing(&mut self, item_id: String) {
        tracing::info!(%item_id, "now playing changed");
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

    pub(super) fn load_playing_episodes(
        &self,
        item_id: String,
        series_id: String,
        season_id: String,
    ) {
        let api = self.api.clone();
        self.spawn(
            "playing_episodes",
            async move {
                api.episodes(&series_id, Some(&season_id), EPISODE_LIMIT)
                    .await
            },
            move |items| Msg::PlayingEpisodes { item_id, items },
        );
    }

    /// One small request per item change, rather than a duration in every
    /// status poll.
    pub(super) fn load_runtime_ticks(&self, item_id: String) {
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
}
