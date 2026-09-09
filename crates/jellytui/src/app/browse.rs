//! Where the cursor is: the rows a level shows, moving through them, and
//! pushing and popping the browse stack.

use super::*;

impl App {
    pub(super) fn rows(&self) -> &[Item] {
        match self.screen {
            Screen::Home => self.home_rows(),
            Screen::Browse => self.stack.last().map_or(&[], |level| &level.items),
            Screen::Search => &self.results.items,
            Screen::Playing => &self.playing_episodes.items,
        }
    }

    pub(crate) fn home_rows(&self) -> &[Item] {
        match self.home_pane {
            HomePane::Resume => &self.resume,
            HomePane::NextUp => &self.next_up,
        }
    }

    pub(crate) fn selected(&self) -> usize {
        match self.screen {
            Screen::Home => self.home_selected,
            Screen::Browse => self.stack.last().map_or(0, |level| level.selected),
            Screen::Search => self.results.selected,
            Screen::Playing => self.playing_episodes.selected,
        }
    }

    /// The looked-up item, only while it still describes what is playing.
    pub(crate) fn current_item(&self) -> Option<&Item> {
        let now_playing = self.now_playing()?;
        self.playing_item
            .as_ref()
            .filter(|(item_id, _)| *item_id == now_playing.item_id)
            .map(|(_, item)| item)
    }

    pub(super) fn selected_item(&self) -> Option<&Item> {
        self.rows().get(self.selected())
    }

    pub(super) fn move_by(&mut self, delta: isize) {
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
    pub(super) fn move_in_grid(&mut self, delta: isize) {
        if self.grid_metrics().is_some() {
            self.move_by(delta);
        }
    }

    pub(super) fn move_to_end(&mut self, end: End) {
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
    pub(super) fn row_step(&self) -> isize {
        self.grid_metrics()
            .and_then(|metrics| isize::try_from(metrics.columns).ok())
            .unwrap_or(1)
    }

    pub(super) fn page_step(&self) -> isize {
        self.grid_metrics()
            .and_then(|metrics| isize::try_from(metrics.page()).ok())
            .unwrap_or(PAGE_JUMP)
    }

    /// The grid the focused screen is drawing, if it is drawing one. Search
    /// stays a list whatever it turned up, because its rows are mixed kinds.
    pub(crate) fn grid_metrics(&self) -> Option<grid::Metrics> {
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

    pub(super) fn body_area(&self) -> Rect {
        view::panes(Rect::new(0, 0, self.viewport.width, self.viewport.height)).body
    }

    pub(crate) fn grid_offset(&self) -> usize {
        match self.screen {
            Screen::Home => self.home_offset,
            Screen::Browse => self.stack.last().map_or(0, |level| level.offset),
            Screen::Search | Screen::Playing => 0,
        }
    }

    /// Scrolls the grid the least that brings the cursor back on screen.
    pub(super) fn rescroll(&mut self) {
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

    pub(super) fn toggle_home_pane(&mut self) {
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

    pub(super) fn enter(&mut self) {
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

    pub(super) fn back(&mut self) {
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

    pub(super) fn open_libraries(&mut self) {
        self.screen = Screen::Browse;
        self.stack.clear();
        self.push("Libraries", Source::Libraries);
    }

    pub(super) fn push(&mut self, title: impl Into<String>, source: Source) {
        self.stack.push(Level::loading(title, source.clone()));
        self.load_level(self.stack.len() - 1, source);
    }

    pub(super) fn refresh(&mut self) {
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

    /// The item whose art the rail is showing. Home has no rail.
    pub(super) fn rail_item(&self) -> Option<&Item> {
        match self.screen {
            Screen::Browse | Screen::Search => self.selected_item(),
            Screen::Home | Screen::Playing => None,
        }
    }
}
