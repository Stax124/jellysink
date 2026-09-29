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
        let (api, requested) = (self.api.clone(), source.clone());
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
                    } => api.episodes(&series_id, &season_id).await,
                }
            },
            move |items| Msg::Level(depth, requested, items),
        );
    }

    /// The per-frame bookkeeping, done before the draw so the draw and the
    /// cover requests agree on what is on screen.
    pub(super) fn prepare_frame(&mut self) {
        self.rescroll();
        self.tick_covers();
    }

    /// Every cover the current screen wants. Recomputed each iteration, so a
    /// resize asks for the new size without anything having to notice it.
    pub(super) fn visible_covers(&self) -> Vec<CoverKey> {
        let (body, font_size) = (self.body_area(), self.covers.font_size());
        match self.screen {
            // Both shelves are on screen, and `grid_metrics` answers for one.
            Screen::Home => HomePane::ALL
                .into_iter()
                .flat_map(|pane| self.grid_keys(self.shelf(pane), self.shelf_metrics(pane)))
                .collect(),
            Screen::Playing => self
                .current_item()
                .and_then(|item| {
                    self.covers
                        .key(item, playing::still_size(body, item, font_size)?)
                })
                .into_iter()
                .collect(),
            _ => match (self.grid_metrics(), self.focused()) {
                (Some(metrics), Some(rows)) => self.grid_keys(rows, Some(metrics)).collect(),
                _ => self
                    .rail_item()
                    .and_then(|item| {
                        self.covers
                            .key(item, rail::cover_size(body, item, font_size)?)
                    })
                    .into_iter()
                    .collect(),
            },
        }
    }

    fn grid_keys<'a>(
        &'a self,
        rows: &'a Rows,
        metrics: Option<grid::Metrics>,
    ) -> impl Iterator<Item = CoverKey> + 'a {
        metrics.into_iter().flat_map(move |metrics| {
            rows.items
                .iter()
                .skip(rows.offset * metrics.columns)
                .take(metrics.page())
                .filter_map(move |item| self.covers.key(item, metrics.cover))
        })
    }

    pub(super) fn tick_covers(&mut self) {
        let wanted = self.visible_covers();
        let moved = wanted != self.wanted_covers;
        self.wanted_covers = wanted;
        // A cover evicted while the cursor stood still would otherwise stay
        // lost until the cursor happened to move.
        if !moved && !self.covers.any_missing(&self.wanted_covers) {
            return;
        }
        let now = tokio::time::Instant::now();
        match self.cover_ready_at {
            // Inside the window: wait it out rather than asking for a row the
            // cursor is only passing through.
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
            let (api, picker, disk) = (self.api.clone(), self.covers.picker(), self.covers.disk());
            self.spawn_msg(async move {
                Some(match cover::fetch(&api, picker, disk, &key).await {
                    Ok(protocol) => Msg::Cover {
                        key,
                        protocol: protocol.map(Box::new),
                    },
                    Err(e) => {
                        let error = format!("{e:#}");
                        tracing::debug!(%error, "cover request failed");
                        Msg::CoverFailed(key)
                    }
                })
            });
        }
        // A batch that asked for nothing — every cover already cached or in
        // flight — costs the server nothing, so it does not close the window.
        if spawned > 0 {
            self.cover_ready_at = Some(tokio::time::Instant::now() + COVER_THROTTLE);
        }
    }

    pub(super) fn set_played(&self, item_id: String, played: bool) {
        tracing::info!(%item_id, played, "marking watched");
        let api = self.api.clone();
        self.spawn_msg(async move {
            Some(
                api.set_played(&item_id, played)
                    .await
                    .map_or_else(Msg::from, |()| Msg::Watched),
            )
        });
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
        self.playing_requested = Some(item_id.clone());
        self.playing_episodes = Rows::default();
        let api = self.api.clone();
        self.spawn_msg(async move {
            let item = api.get_item(&item_id).await.and_then(|value| {
                Item::deserialize(&value).wrap_err_with(|| format!("decoding item {item_id}"))
            });
            Some(match item {
                Ok(item) => Msg::PlayingItem {
                    item_id,
                    item: Box::new(item),
                },
                Err(e) => e.into(),
            })
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
            async move { api.episodes(&series_id, &season_id).await },
            move |items| Msg::PlayingEpisodes { item_id, items },
        );
    }
}
