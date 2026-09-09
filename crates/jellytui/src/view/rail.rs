//! The detail rail: cover and metadata for whichever row holds the cursor.

use crate::cover::{self, CoverKey, Covers};
use jellysink_core::jellyfin::model::Item;
use jellysink_core::ticks::ticks_to_seconds;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect, Size};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};
use ratatui_image::FontSize;
use ratatui_image::Image;

/// Below this the rail would leave the list too narrow to read, so the screen
/// stays the full-width list it is today.
const MIN_BODY_WIDTH: u16 = 90;

/// Splits a body area into the list and the rail beside it. Half each: a row
/// is a line of text and elides gracefully, while the cover beside it is the
/// thing worth the width.
pub(crate) fn split(body: Rect) -> (Rect, Option<Rect>) {
    if body.width < MIN_BODY_WIDTH {
        return (body, None);
    }
    let [list, rail] = Layout::horizontal([Constraint::Fill(1), Constraint::Fill(1)]).areas(body);
    (list, Some(rail))
}

/// The box the cover occupies inside the rail. Height is capped at three
/// fifths so a 2:3 poster leaves room for the text under it; a 16:9 still is
/// bounded by the width first anyway.
pub(crate) fn cover_rect(rail: Rect, item: &Item, font_size: FontSize) -> Rect {
    let inner = block().inner(rail);
    cover::fit(
        inner,
        cover::primary_aspect(item),
        font_size,
        (inner.height * 3 / 5).max(1),
    )
}

/// The size a rail cover is encoded for, or `None` when this body has no rail.
/// The fetch and the renderer both come through [`cover_rect`], so what is
/// downloaded is the size it is drawn at.
pub(crate) fn cover_size(body: Rect, item: &Item, font_size: FontSize) -> Option<Size> {
    Some(cover_rect(split(body).1?, item, font_size).as_size())
}

fn block() -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(" Details ")
}

pub(crate) fn render(frame: &mut Frame, rail: Rect, item: Option<&Item>, covers: &Covers) {
    let block = block();
    let inner = block.inner(rail);
    frame.render_widget(block, rail);
    let Some(item) = item else {
        return;
    };

    let cover = cover_rect(rail, item, covers.font_size());
    if let Some(protocol) = CoverKey::primary(item, cover.as_size())
        .as_ref()
        .and_then(|key| covers.protocol(key))
    {
        frame.render_widget(Image::new(protocol), cover);
    }

    let [_, details] = Layout::vertical([
        Constraint::Length(cover.height.saturating_add(1)),
        Constraint::Fill(1),
    ])
    .areas(inner);
    frame.render_widget(
        Paragraph::new(lines(item)).wrap(Wrap { trim: true }),
        details,
    );
}

fn lines(item: &Item) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(Span::styled(
            item.label(),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(meta(item), Style::default().fg(super::DIM))),
    ];
    if !item.genres.is_empty() {
        lines.push(Line::from(Span::styled(
            item.genres.join(" · "),
            Style::default().fg(super::DIM),
        )));
    }
    if let Some(rating) = item.community_rating {
        lines.push(Line::from(Span::styled(
            format!("★ {rating:.1}"),
            Style::default().fg(super::ACCENT),
        )));
    }
    lines.push(Line::default());
    if let Some(overview) = &item.overview {
        lines.push(Line::from(Span::styled(
            overview.clone(),
            Style::default().fg(super::DIM),
        )));
    }
    lines
}

/// The one line under the title, skipping whatever the server did not send.
/// A folder counts its children where a playable item gives its runtime: a
/// series' `RunTimeTicks` is the nominal length of one episode, so printing it
/// beside a season list claims the whole show is over in twenty-four minutes.
pub(crate) fn meta(item: &Item) -> String {
    let year = item.production_year.map(|year| year.to_string());
    let left = item.unplayed_count().map(|count| format!("{count} left"));
    let episodes = item
        .recursive_item_count
        .map(|count| counted(count, "episode"));
    let parts = match item.kind() {
        "Series" => vec![
            year,
            item.child_count.map(|count| counted(count, "season")),
            episodes,
            left,
            item.official_rating.clone(),
        ],
        "Season" => vec![item.series_name.clone(), year, episodes, left],
        _ => vec![
            item.series_name.clone(),
            year,
            item.run_time_ticks
                .filter(|ticks| *ticks > 0)
                .map(|ticks| format!("{} min", (ticks_to_seconds(ticks) / 60.0).round() as i64)),
            item.official_rating.clone(),
        ],
    };
    parts.into_iter().flatten().collect::<Vec<_>>().join(" · ")
}

fn counted(count: i64, noun: &str) -> String {
    let plural = if count == 1 { "" } else { "s" };
    format!("{count} {noun}{plural}")
}

#[cfg(test)]
#[path = "rail_test.rs"]
mod tests;
