//! The browse stack: where the user is, and what Enter does next.

use crate::jellyfin::model::Item;

/// One screen's worth of rows plus the cursor in them. Levels stack, so going
/// back restores the position rather than re-fetching.
#[derive(Debug, Clone)]
pub(super) struct Level {
    pub(super) title: String,
    pub(super) source: Source,
    pub(super) items: Vec<Item>,
    pub(super) selected: usize,
    /// First visible row when this level is drawn as a grid. A list keeps its
    /// own scroll inside `ListState`, which is why only the grid needs it.
    pub(super) offset: usize,
    pub(super) loading: bool,
}

/// What produced a level's rows, which is also how it is reloaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Source {
    Libraries,
    Folder(String),
    Seasons(String),
    Episodes {
        series_id: String,
        season_id: String,
    },
}

impl Level {
    pub(super) fn loading(title: impl Into<String>, source: Source) -> Self {
        Self {
            title: title.into(),
            source,
            items: Vec::new(),
            selected: 0,
            offset: 0,
            loading: true,
        }
    }

    pub(super) fn fill(&mut self, items: Vec<Item>) {
        self.selected = self.selected.min(items.len().saturating_sub(1));
        self.offset = 0;
        self.items = items;
        self.loading = false;
    }

    pub(super) fn move_by(&mut self, delta: isize) {
        if self.items.is_empty() {
            self.selected = 0;
            return;
        }
        let last = self.items.len() - 1;
        self.selected = self.selected.saturating_add_signed(delta).min(last);
    }

    pub(super) fn move_to_end(&mut self, end: End) {
        self.selected = match end {
            End::Top => 0,
            End::Bottom => self.items.len().saturating_sub(1),
        };
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum End {
    Top,
    Bottom,
}

/// Whether a level's rows are poster-shaped enough to be worth a grid.
/// Decided by kind rather than by [`Source`], so a folder full of movies gets
/// the grid whichever route reached it.
pub(super) fn is_grid(items: &[Item]) -> bool {
    items
        .first()
        .is_some_and(|item| matches!(item.kind(), "Series" | "Season" | "Movie" | "BoxSet"))
}

/// The level Enter on `item` should push, if it is not something to play.
pub(super) fn descend(item: &Item) -> Option<Source> {
    match item.kind() {
        "Series" => Some(Source::Seasons(item.id.clone())),
        "Season" => item.series_id.as_ref().map(|series_id| Source::Episodes {
            series_id: series_id.clone(),
            season_id: item.id.clone(),
        }),
        _ if item.is_container() => Some(Source::Folder(item.id.clone())),
        _ => None,
    }
}

#[cfg(test)]
#[path = "nav_test.rs"]
mod tests;
