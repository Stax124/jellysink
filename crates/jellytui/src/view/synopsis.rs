//! The band under an episode grid: the selected episode's meta line and
//! synopsis, which a tile's one caption row has no room for.

use super::{DIM, panel, rail};
use jellysink_core::jellyfin::model::Item;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

/// The borders, the meta line and four lines of synopsis; a longer one is clipped.
const HEIGHT: u16 = 7;
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// The grid's area above the band's. `App` sizes the grid from the first, so
/// the tiles and the cover requests never reach under the band.
pub(crate) fn split(body: Rect) -> [Rect; 2] {
    Layout::vertical([Constraint::Fill(1), Constraint::Length(HEIGHT)]).areas(body)
}

pub(super) fn render(frame: &mut Frame, area: Rect, item: &Item) {
    let title = Span::styled(
        format!(" {} ", item.label()),
        Style::default().add_modifier(Modifier::BOLD),
    );
    let mut lines = vec![Line::styled(meta(item), Style::default().fg(DIM))];
    lines.extend(item.overview.as_deref().map(Line::raw));
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel(title))
            .wrap(Wrap { trim: true }),
        area,
    );
}

/// Series and year are the same on every episode of a season, so they are left
/// to the title bar.
fn meta(item: &Item) -> String {
    let user_data = item.user_data.as_ref();
    [
        rail::runtime(item),
        item.community_rating.map(|rating| format!("★ {rating:.1}")),
        user_data
            .is_some_and(|user_data| user_data.is_favorite)
            .then(|| "♥".to_string()),
        user_data
            .and_then(|user_data| user_data.last_played_date.as_deref())
            .and_then(calendar_day)
            .map(|day| format!("watched {day}")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ")
}

/// `1 Sep 2026` from the date part of an ISO 8601 timestamp.
fn calendar_day(timestamp: &str) -> Option<String> {
    let mut parts = timestamp.get(..10)?.split('-');
    let year: u32 = parts.next()?.parse().ok()?;
    let month: usize = parts.next()?.parse().ok()?;
    let day: u32 = parts.next()?.parse().ok()?;
    let month = MONTHS.get(month.checked_sub(1)?)?;
    Some(format!("{day} {month} {year}"))
}

#[cfg(test)]
#[path = "synopsis_test.rs"]
mod tests;
