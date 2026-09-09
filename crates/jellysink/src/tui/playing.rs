//! The Playing screen: the episode's own still, what it is, and the rest of
//! the season under it.

use super::app::App;
use super::cover::{self, CoverKey};
use super::rail;
use super::ui::{self, ACCENT, DIM};
use jellysink_core::jellyfin::model::Item;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect, Size};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui_image::FontSize;
use ratatui_image::Image;

const STILL_WIDTH: u16 = 34;
/// Narrower than this and the synopsis beside the still has no measure left,
/// so the screen becomes text only.
const MIN_BANNER_WIDTH: u16 = 70;
/// The gap between the still and the text beside it.
const GUTTER: u16 = 2;

fn block() -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(" Playing ")
}

/// The still's box, or `None` when the body is too narrow to carry one. Half
/// the height is the cap that leaves the season its own half; a 16:9 still is
/// bounded by the width first anyway.
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

/// The size the still is encoded for. The fetch and the renderer both come
/// through [`still_rect`], so what is downloaded is the size it is drawn at.
pub(super) fn still_size(body: Rect, item: &Item, font_size: FontSize) -> Option<Size> {
    Some(still_rect(body, item, font_size)?.as_size())
}

pub(super) fn render(app: &App, frame: &mut Frame, area: Rect) {
    let block = block();
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(now_playing) = app.now_playing() else {
        frame.render_widget(
            Paragraph::new(Span::styled(
                match (app.player_polled, app.player.is_some()) {
                    (false, _) => "checking for jellysink…",
                    (true, true) => "nothing playing",
                    (true, false) => "jellysink not connected",
                },
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

    let detail = match still {
        Some(rect) => {
            if let Some(protocol) = item
                .and_then(|item| CoverKey::primary(item, rect.as_size()))
                .as_ref()
                .and_then(|key| app.covers.protocol(key))
            {
                frame.render_widget(Image::new(protocol), rect);
            }
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
    let Some(item) = item else {
        return vec![Line::from(Span::styled(
            display_title.to_string(),
            Style::default().add_modifier(Modifier::BOLD),
        ))];
    };
    let mut lines = vec![
        Line::from(Span::styled(
            item.label(),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(rail::meta(item), Style::default().fg(DIM))),
    ];
    if let Some(rating) = item.community_rating {
        lines.push(Line::from(Span::styled(
            format!("★ {rating:.1}"),
            Style::default().fg(ACCENT),
        )));
    }
    lines.push(Line::default());
    if let Some(overview) = &item.overview {
        lines.push(Line::from(Span::styled(
            overview.clone(),
            Style::default().fg(DIM),
        )));
    }
    lines
}

fn render_episodes(frame: &mut Frame, area: Rect, app: &App, playing_id: &str) {
    let level = &app.playing_episodes;
    if level.items.is_empty() {
        return;
    }
    // A rule rather than a box: the season belongs to the banner above it.
    let rule = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(DIM));
    let list_area = rule.inner(area);
    frame.render_widget(rule, area);
    let rows: Vec<ListItem> = level
        .items
        .iter()
        .map(|item| {
            let row = ui::row(item);
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
    state.select(Some(level.selected));
    frame.render_stateful_widget(
        List::new(rows).highlight_style(
            Style::default()
                .fg(ACCENT)
                .add_modifier(Modifier::REVERSED | Modifier::BOLD),
        ),
        list_area,
        &mut state,
    );
}

#[cfg(test)]
#[path = "playing_test.rs"]
mod tests;
