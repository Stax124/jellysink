//! The Playing screen: what the daemon is on, at the size the artwork
//! deserves, with the rest of the season under it.

use super::app::App;
use super::cover;
use super::rail;
use super::ui::{self, ACCENT, DIM};
use crate::jellyfin::model::Item;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect, Size};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui_image::FontSize;
use ratatui_image::Image;

const POSTER_WIDTH: u16 = 34;
/// The poster column is always poster-shaped: an episode borrows its series'
/// cover rather than showing its own 16:9 still.
const POSTER_ASPECT: f32 = 2.0 / 3.0;
/// Title, meta, rating, a blank, and four lines of synopsis.
const DETAIL_HEIGHT: u16 = 8;

fn block() -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
}

fn columns(body: Rect) -> (Rect, Rect) {
    let [poster, detail] =
        Layout::horizontal([Constraint::Length(POSTER_WIDTH), Constraint::Fill(1)]).areas(body);
    (poster, detail)
}

/// The poster's box, which is also the size it is encoded for — the fetch and
/// the renderer both come through here.
pub(super) fn poster_rect(body: Rect, font_size: FontSize) -> Rect {
    let inner = block().inner(columns(body).0);
    cover::fit(inner, POSTER_ASPECT, font_size, inner.height)
}

pub(super) fn poster_size(body: Rect, font_size: FontSize) -> Size {
    poster_rect(body, font_size).as_size()
}

pub(super) fn render(app: &App, frame: &mut Frame, area: Rect) {
    let (poster_area, detail_area) = columns(area);
    frame.render_widget(block(), poster_area);
    frame.render_widget(block().title(" Playing "), detail_area);

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
            block().inner(detail_area),
        );
        return;
    };

    let item = app.current_item();
    if let Some(protocol) = item
        .and_then(|item| cover::poster_key(item, poster_size(area, app.covers.font_size())))
        .as_ref()
        .and_then(|key| app.covers.protocol(key))
    {
        frame.render_widget(
            Image::new(protocol),
            poster_rect(area, app.covers.font_size()),
        );
    }

    let inner = block().inner(detail_area);
    let [detail, episodes] =
        Layout::vertical([Constraint::Length(DETAIL_HEIGHT), Constraint::Fill(1)]).areas(inner);
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
        area,
        &mut state,
    );
}

#[cfg(test)]
#[path = "playing_test.rs"]
mod tests;
