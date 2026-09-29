//! Where the cursor is: the rows a level shows, moving through them, and
//! pushing and popping the browse stack.

use super::*;

impl App {
    pub(crate) fn shelf(&self, pane: HomePane) -> &Rows {
        &self.shelves[pane as usize]
    }

    pub(crate) fn shelf_mut(&mut self, pane: HomePane) -> &mut Rows {
        &mut self.shelves[pane as usize]
    }

    /// The rows the cursor keys move through on the current screen.
    pub(super) fn focused(&self) -> Option<&Rows> {
        match self.screen {
            Screen::Home => Some(self.shelf(self.home_pane)),
            Screen::Browse => self.stack.last().map(|level| &level.rows),
            Screen::Search => Some(&self.results),
            Screen::Playing => Some(&self.playing_episodes),
            Screen::Logs => None,
        }
    }

    fn focused_mut(&mut self) -> Option<&mut Rows> {
        match self.screen {
            Screen::Home => Some(self.shelf_mut(self.home_pane)),
            Screen::Browse => self.stack.last_mut().map(|level| &mut level.rows),
            Screen::Search => Some(&mut self.results),
            Screen::Playing => Some(&mut self.playing_episodes),
            Screen::Logs => None,
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
        self.focused()?.selected_item()
    }

    pub(super) fn move_by(&mut self, delta: isize) {
        if let Some(rows) = self.focused_mut() {
            rows.move_by(delta);
        }
    }

    /// Only a grid has a second axis. A list ignores these rather than making
    /// the arrows a second, unadvertised way to do Esc and Enter.
    pub(super) fn move_in_grid(&mut self, delta: isize) {
        if self.grid_metrics().is_some() {
            self.move_by(delta);
        }
    }

    pub(super) fn move_to_end(&mut self, end: End) {
        if let Some(rows) = self.focused_mut() {
            rows.move_to_end(end);
        }
    }

    /// Up and down. A Home shelf is a single row, so there they change which
    /// shelf has focus rather than moving along one.
    pub(super) fn move_vertically(&mut self, direction: isize) {
        if self.screen == Screen::Home {
            self.home_pane = if direction > 0 {
                HomePane::NextUp
            } else {
                HomePane::Resume
            };
            return;
        }
        self.move_by(direction * self.row_step());
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
        match self.screen {
            Screen::Home => self.shelf_metrics(self.home_pane),
            Screen::Browse => {
                let items = &self.stack.last()?.rows.items;
                if !nav::is_grid(items) {
                    return None;
                }
                grid::metrics_for(
                    self.body_area(),
                    items,
                    self.covers.font_size(),
                    grid::TARGET_ROWS,
                )
            }
            Screen::Search | Screen::Playing | Screen::Logs => None,
        }
    }

    /// A shelf's tiles whether or not it has focus: the covers in the other
    /// one are on screen too and still have to be asked for.
    pub(crate) fn shelf_metrics(&self, pane: HomePane) -> Option<grid::Metrics> {
        grid::metrics_for(
            view::body::shelves(self.body_area())[pane as usize],
            &self.shelf(pane).items,
            self.covers.font_size(),
            grid::SHELF_ROWS,
        )
    }

    pub(super) fn body_area(&self) -> Rect {
        view::panes(Rect::new(0, 0, self.viewport.width, self.viewport.height)).body
    }

    /// Scrolls each grid on screen the least that brings its cursor back into
    /// view, whether a key, a reload or a resize moved it out.
    pub(super) fn rescroll(&mut self) {
        let scroll = |rows: &mut Rows, metrics: grid::Metrics| {
            rows.offset = grid::scroll_to(rows.offset, rows.selected, &metrics);
        };
        if self.screen == Screen::Home {
            for pane in HomePane::ALL {
                if let Some(metrics) = self.shelf_metrics(pane) {
                    scroll(self.shelf_mut(pane), metrics);
                }
            }
        } else if let Some(metrics) = self.grid_metrics()
            && let Some(rows) = self.focused_mut()
        {
            scroll(rows, metrics);
        }
    }

    pub(super) fn toggle_shelf(&mut self) {
        if self.screen == Screen::Home {
            self.home_pane = match self.home_pane {
                HomePane::Resume => HomePane::NextUp,
                HomePane::NextUp => HomePane::Resume,
            };
        }
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

    pub(super) fn toggle_watched(&mut self) {
        let Some((item_id, played)) = self
            .selected_item()
            .map(|item| (item.id.clone(), item.played()))
        else {
            return;
        };
        self.set_played(item_id, !played);
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
            Screen::Home | Screen::Logs => {}
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
        self.reload_current_screen();
        self.poll_player();
    }

    pub(super) fn reload_current_screen(&mut self) {
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
                if let Some(item_id) = self
                    .now_playing()
                    .map(|now_playing| now_playing.item_id.clone())
                {
                    self.load_playing(item_id);
                }
            }
            Screen::Logs => {}
        }
    }

    /// The item whose art the rail is showing. Home has no rail.
    pub(super) fn rail_item(&self) -> Option<&Item> {
        match self.screen {
            Screen::Browse | Screen::Search => self.selected_item(),
            Screen::Home | Screen::Playing | Screen::Logs => None,
        }
    }
}
