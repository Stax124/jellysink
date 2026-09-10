//! Where the cursor is: the rows a level shows, moving through them, and
//! pushing and popping the browse stack.

use super::*;

/// One Home shelf: a row of tiles and the cursor in it, which it keeps while
/// the other shelf has focus.
#[derive(Debug, Default)]
pub(crate) struct Shelf {
    pub(crate) items: Vec<Item>,
    pub(crate) selected: usize,
    pub(crate) offset: usize,
}

impl Shelf {
    pub(crate) fn fill(&mut self, items: Vec<Item>) {
        self.selected = self.selected.min(items.len().saturating_sub(1));
        self.offset = 0;
        self.items = items;
    }
}

impl App {
    pub(super) fn rows(&self) -> &[Item] {
        match self.screen {
            Screen::Home => &self.shelf(self.home_pane).items,
            Screen::Browse => self.stack.last().map_or(&[], |level| &level.items),
            Screen::Search => &self.results.items,
            Screen::Playing => &self.playing_episodes.items,
            Screen::Logs => &[],
        }
    }

    pub(crate) fn shelf(&self, pane: HomePane) -> &Shelf {
        match pane {
            HomePane::Resume => &self.resume,
            HomePane::NextUp => &self.next_up,
        }
    }

    pub(super) fn shelf_mut(&mut self, pane: HomePane) -> &mut Shelf {
        match pane {
            HomePane::Resume => &mut self.resume,
            HomePane::NextUp => &mut self.next_up,
        }
    }

    pub(crate) fn selected(&self) -> usize {
        match self.screen {
            Screen::Home => self.shelf(self.home_pane).selected,
            Screen::Browse => self.stack.last().map_or(0, |level| level.selected),
            Screen::Search => self.results.selected,
            Screen::Playing => self.playing_episodes.selected,
            Screen::Logs => 0,
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
                let shelf = self.shelf_mut(self.home_pane);
                let last = shelf.items.len().saturating_sub(1);
                shelf.selected = shelf.selected.saturating_add_signed(delta).min(last);
            }
            Screen::Browse => {
                if let Some(level) = self.stack.last_mut() {
                    level.move_by(delta);
                }
            }
            Screen::Search => self.results.move_by(delta),
            Screen::Playing => self.playing_episodes.move_by(delta),
            Screen::Logs => {}
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
                let shelf = self.shelf_mut(self.home_pane);
                shelf.selected = match end {
                    End::Top => 0,
                    End::Bottom => shelf.items.len().saturating_sub(1),
                };
            }
            Screen::Browse => {
                if let Some(level) = self.stack.last_mut() {
                    level.move_to_end(end);
                }
            }
            Screen::Search => self.results.move_to_end(end),
            Screen::Playing => self.playing_episodes.move_to_end(end),
            Screen::Logs => {}
        }
        self.rescroll();
    }

    /// Up and down. A Home shelf is a single row, so there they change which
    /// shelf has focus rather than moving along one.
    pub(super) fn move_vertically(&mut self, direction: isize) {
        if self.screen == Screen::Home {
            self.focus_shelf(direction);
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
    /// stays a list whatever it turned up, because its rows are mixed kinds,
    /// and the Playing screen draws one still of its own.
    pub(crate) fn grid_metrics(&self) -> Option<grid::Metrics> {
        match self.screen {
            Screen::Home => self.shelf_metrics(self.home_pane),
            Screen::Browse => {
                let first = self.rows().first()?;
                nav::is_grid(self.rows()).then(|| {
                    grid::metrics(
                        grid::inner(self.body_area()),
                        cover::primary_aspect(first),
                        self.covers.font_size(),
                        grid::TARGET_ROWS,
                    )
                })
            }
            Screen::Search | Screen::Playing | Screen::Logs => None,
        }
    }

    /// A shelf's tiles whether or not it has focus: the covers in the other
    /// one are on screen too and still have to be asked for.
    pub(crate) fn shelf_metrics(&self, pane: HomePane) -> Option<grid::Metrics> {
        let first = self.shelf(pane).items.first()?;
        Some(grid::metrics(
            grid::inner(view::body::shelf_rect(self.body_area(), pane)),
            cover::primary_aspect(first),
            self.covers.font_size(),
            grid::SHELF_ROWS,
        ))
    }

    pub(super) fn body_area(&self) -> Rect {
        view::panes(Rect::new(0, 0, self.viewport.width, self.viewport.height)).body
    }

    pub(crate) fn grid_offset(&self) -> usize {
        match self.screen {
            Screen::Home => self.shelf(self.home_pane).offset,
            Screen::Browse => self.stack.last().map_or(0, |level| level.offset),
            Screen::Search | Screen::Playing | Screen::Logs => 0,
        }
    }

    /// Scrolls the grid the least that brings the cursor back on screen.
    pub(super) fn rescroll(&mut self) {
        let Some(metrics) = self.grid_metrics() else {
            return;
        };
        let offset = grid::scroll_to(self.grid_offset(), self.selected(), &metrics);
        match self.screen {
            Screen::Home => self.shelf_mut(self.home_pane).offset = offset,
            Screen::Browse => {
                if let Some(level) = self.stack.last_mut() {
                    level.offset = offset;
                }
            }
            Screen::Search | Screen::Playing | Screen::Logs => {}
        }
    }

    /// Moves the focus between the shelves. Each keeps its own cursor, so
    /// coming back lands where it was left.
    pub(super) fn focus_shelf(&mut self, direction: isize) {
        if self.screen != Screen::Home {
            return;
        }
        self.home_pane = match (self.home_pane, direction) {
            (HomePane::Resume, 1) => HomePane::NextUp,
            (HomePane::NextUp, -1) => HomePane::Resume,
            (pane, _) => pane,
        };
    }

    pub(super) fn toggle_shelf(&mut self) {
        self.focus_shelf(if self.home_pane == HomePane::Resume {
            1
        } else {
            -1
        });
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
            Screen::Logs => {}
        }
        self.poll_player();
    }

    /// The item whose art the rail is showing. Home has no rail.
    pub(super) fn rail_item(&self) -> Option<&Item> {
        match self.screen {
            Screen::Browse | Screen::Search => self.selected_item(),
            Screen::Home | Screen::Playing | Screen::Logs => None,
        }
    }
}
