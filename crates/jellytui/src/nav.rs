//! The browse stack: where the user is, and what Enter does next.

use jellysink_core::jellyfin::model::Item;

/// Items and the cursor in them, which every screen moves through alike.
#[derive(Debug, Clone, Default)]
pub(super) struct Rows {
    pub(super) items: Vec<Item>,
    pub(super) selected: usize,
    /// First visible row when drawn as a grid; a list scrolls inside `ListState`.
    pub(super) offset: usize,
    pub(super) loading: bool,
}

impl Rows {
    pub(super) fn loading() -> Self {
        Self {
            loading: true,
            ..Self::default()
        }
    }

    pub(super) fn fill(&mut self, items: Vec<Item>) {
        self.selected = self.selected.min(items.len().saturating_sub(1));
        self.offset = 0;
        self.items = items;
        self.loading = false;
    }

    pub(super) fn selected_item(&self) -> Option<&Item> {
        self.items.get(self.selected)
    }

    pub(super) fn move_by(&mut self, delta: isize) {
        let last = self.items.len().saturating_sub(1);
        self.selected = self.selected.saturating_add_signed(delta).min(last);
    }

    pub(super) fn move_to_end(&mut self, end: End) {
        self.selected = match end {
            End::Top => 0,
            End::Bottom => self.items.len().saturating_sub(1),
        };
    }
}

/// One step of the browse stack. Levels stack, so going back restores the
/// position rather than re-fetching.
#[derive(Debug, Clone)]
pub(super) struct Level {
    pub(super) title: String,
    pub(super) source: Source,
    pub(super) rows: Rows,
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
            rows: Rows::loading(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum End {
    Top,
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    List,
    Grid,
    /// A grid over the selected episode's synopsis.
    Episodes,
}

/// Which view a level's rows get. Only the first row is asked, so kinds that
/// share a screen must answer alike.
pub(super) fn mode(items: &[Item]) -> Mode {
    match items.first().map(Item::kind) {
        Some("Series" | "Season" | "Movie" | "BoxSet" | "CollectionFolder" | "UserView") => {
            Mode::Grid
        }
        Some("Episode") => Mode::Episodes,
        _ => Mode::List,
    }
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
