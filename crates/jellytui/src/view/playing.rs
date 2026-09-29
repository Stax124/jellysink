//! The Playing screen: the episode's own still, what it is, and the rest of
//! the season under it.

use super::{ACCENT, DIM, SELECTED, idle_text, panel};
use crate::app::App;
use crate::cover;
use crate::view::rail;
use jellysink_core::jellyfin::model::Item;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect, Size};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui_image::FontSize;

const STILL_WIDTH: u16 = 34;
/// Narrower than this and the synopsis beside the still has no measure left,
/// so the screen becomes text only.
const MIN_BANNER_WIDTH: u16 = 70;
const GUTTER: u16 = 2;

fn block() -> Block<'static> {
    panel(" Playing ")
}

/// The still's box, or `None` when the body is too narrow to carry one. Capped
/// at half the height, which leaves the season its own half.
fn still_rect(body: Rect, item: &Item, font_size: FontSize) -> Option<Rect> {
    if body.width < MIN_BANNER_WIDTH {
        return None;
    }
    let inner = block().inner(body);
    let column = Rect {
        width: STILL_WIDTH.min(inner.width),
        ..inner
    };
    Some(cover::fit(
        column,
        cover::primary_aspect(item),
        font_size,
        (inner.height / 2).max(1),
    ))
}

pub(crate) fn still_size(body: Rect, item: &Item, font_size: FontSize) -> Option<Size> {
    Some(still_rect(body, item, font_size)?.as_size())
}

pub(crate) fn render(app: &App, frame: &mut Frame, area: Rect) {
    let block = block();
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(now_playing) = app.now_playing() else {
        frame.render_widget(
            Paragraph::new(Span::styled(
                idle_text(&app.daemon),
                Style::default().fg(DIM),
            )),
            inner,
        );
        return;
    };

    let item = app.current_item();
    let font_size = app.covers.font_size();
    let still = item.and_then(|item| still_rect(area, item, font_size));
    let [banner, episodes] = Layout::vertical([
        Constraint::Length(still.map_or(1, |rect| rect.height)),
        Constraint::Fill(1),
    ])
    .areas(inner);

    let detail = match still.zip(item) {
        Some((rect, item)) => {
            app.covers.draw(frame, item, rect);
            let [_, detail] = Layout::horizontal([
                Constraint::Length(STILL_WIDTH + GUTTER),
                Constraint::Fill(1),
            ])
            .areas(banner);
            detail
        }
        None => banner,
    };
    frame.render_widget(
        Paragraph::new(detail_lines(item, &now_playing.title)).wrap(Wrap { trim: true }),
        detail,
    );
    render_episodes(frame, episodes, app, &now_playing.item_id);
}

/// The daemon's `display_title` stands in until the item lookup lands, so the
/// screen names what is playing from the first frame.
fn detail_lines(item: Option<&Item>, display_title: &str) -> Vec<Line<'static>> {
    match item {
        Some(item) => rail::lines(item, false),
        None => vec![Line::from(Span::styled(
            display_title.to_string(),
            Style::default().add_modifier(Modifier::BOLD),
        ))],
    }
}

fn render_episodes(frame: &mut Frame, area: Rect, app: &App, playing_id: &str) {
    let episodes = &app.playing_episodes;
    if episodes.items.is_empty() {
        return;
    }
    // A rule rather than a box: the season belongs to the banner above it.
    let rule = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(DIM));
    let list_area = rule.inner(area);
    frame.render_widget(rule, area);
    let rows: Vec<ListItem> = episodes
        .items
        .iter()
        .map(|item| {
            let row = super::body::row(item);
            // The cursor is reverse video; the episode actually playing is
            // accented, so the two can be on different rows and both read.
            if item.id == playing_id {
                row.style(Style::default().fg(ACCENT))
            } else {
                row
            }
        })
        .collect();
    let mut state = ListState::default();
    state.select(Some(episodes.selected));
    frame.render_stateful_widget(
        List::new(rows).highlight_style(SELECTED),
        list_area,
        &mut state,
    );
}

#[cfg(test)]
#[path = "playing_test.rs"]
mod tests;
