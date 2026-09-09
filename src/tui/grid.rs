//! The cover grid: how many tiles fit, which of them are on screen, and the
//! tiles themselves.

use super::cover::{self, CoverKey, Covers};
use super::rail;
use super::ui::{ACCENT, DIM, to_width};
use crate::jellyfin::model::Item;
use ratatui::Frame;
use ratatui::layout::{Rect, Size};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui_image::FontSize;
use ratatui_image::Image;

const GAP: u16 = 1;
/// The per-tile progress track. An eighth block (`▔`) is a hairline at any
/// font size; an upper half block is four times the height and still sits
/// against the cover above it rather than floating in its own row.
const TRACK: &str = "▀";
/// Under every cover: the shelf rule, the name, and the year/runtime line.
const LABEL_HEIGHT: u16 = 3;

/// What a tile would like to be before the columns are evened out across the
/// area. A 2:3 poster stays legible narrow; a 16:9 still does not.
fn preferred_tile_width(aspect: f32) -> u16 {
    if aspect > 1.0 { 26 } else { 18 }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Metrics {
    pub(super) columns: usize,
    pub(super) rows: usize,
    tile: Size,
    cover: Size,
}

impl Metrics {
    pub(super) fn cover_size(&self) -> Size {
        self.cover
    }

    pub(super) fn page(&self) -> usize {
        self.columns * self.rows
    }
}

/// Tile geometry for the grid's *inner* area — see [`inner`].
pub(super) fn metrics(area: Rect, aspect: f32, font_size: FontSize) -> Metrics {
    let preferred = preferred_tile_width(aspect);
    let columns = (area.width.saturating_add(GAP) / preferred.saturating_add(GAP)).max(1);
    let tile_width = ((area.width.saturating_sub(GAP * (columns - 1))) / columns).max(1);
    let cover_width = tile_width.saturating_sub(2).max(1);
    let cover_height = cover::rows_for(cover_width, aspect, font_size);
    let tile_height = cover_height + LABEL_HEIGHT;
    Metrics {
        columns: usize::from(columns),
        rows: usize::from((area.height / tile_height).max(1)),
        tile: Size::new(tile_width, tile_height),
        cover: Size::new(cover_width, cover_height),
    }
}

/// Scrolls by the least that brings the selection back on screen, which is
/// what stops the grid jumping a whole page when the cursor moves up one row.
pub(super) fn scroll_to(offset: usize, selected: usize, metrics: &Metrics) -> usize {
    let row = selected / metrics.columns;
    if row < offset {
        row
    } else if row >= offset + metrics.rows {
        row + 1 - metrics.rows
    } else {
        offset
    }
}

pub(super) fn inner(area: Rect) -> Rect {
    block(Line::default()).inner(area)
}

fn block(title: Line<'_>) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .border_type(BorderType::Rounded)
        .title(title)
}

pub(super) fn render(
    frame: &mut Frame,
    area: Rect,
    title: Line<'_>,
    items: &[Item],
    selected: usize,
    offset: usize,
    covers: &Covers,
) {
    let block = block(title);
    let area_inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(first) = items.first() else {
        frame.render_widget(
            Paragraph::new(Span::styled("nothing here", Style::default().fg(DIM))),
            area_inner,
        );
        return;
    };

    let metrics = metrics(area_inner, cover::primary_aspect(first), covers.font_size());
    let start = offset * metrics.columns;
    for (index, item) in items.iter().enumerate().skip(start).take(metrics.page()) {
        let slot = index - start;
        let column = u16::try_from(slot % metrics.columns).unwrap_or(0);
        let row = u16::try_from(slot / metrics.columns).unwrap_or(0);
        let tile = Rect {
            x: area_inner.x + column * (metrics.tile.width + GAP),
            y: area_inner.y + row * metrics.tile.height,
            width: metrics.tile.width,
            height: metrics.tile.height,
        };
        if tile.bottom() > area_inner.bottom() || tile.right() > area_inner.right() {
            continue;
        }
        render_tile(frame, tile, &metrics, item, index == selected, covers);
    }
}

fn render_tile(
    frame: &mut Frame,
    tile: Rect,
    metrics: &Metrics,
    item: &Item,
    selected: bool,
    covers: &Covers,
) {
    let cover = Rect {
        x: tile.x + 1,
        y: tile.y,
        width: metrics.cover.width,
        height: metrics.cover.height,
    };
    if let Some(protocol) = CoverKey::primary(item, cover.as_size())
        .as_ref()
        .and_then(|key| covers.protocol(key))
    {
        frame.render_widget(Image::new(protocol), cover);
    }

    let label_style = if selected {
        Style::default()
            .fg(Color::Black)
            .bg(ACCENT)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().add_modifier(Modifier::BOLD)
    };
    let rows = [
        watched_rule(item, cover.width),
        Line::from(Span::styled(
            to_width(&item.label(), cover.width),
            label_style,
        )),
        Line::from(Span::styled(
            to_width(&caption_meta(item), cover.width),
            Style::default().fg(DIM),
        )),
    ];
    for (offset, span) in rows.into_iter().enumerate() {
        let y = cover.y + cover.height + u16::try_from(offset).unwrap_or(0);
        if y >= tile.bottom() {
            break;
        }
        frame.render_widget(
            Paragraph::new(span),
            Rect {
                x: cover.x,
                y,
                width: cover.width,
                height: 1,
            },
        );
    }
}

/// The shelf rule under a cover, doubling as the progress bar a partly
/// watched item would show its percentage for in a list. Selection is carried
/// by the caption's highlight, so this is free to mean one thing only.
fn watched_rule(item: &Item, width: u16) -> Line<'static> {
    let fraction = if item.played() {
        1.0
    } else {
        item.watched_fraction().unwrap_or(0.0)
    };
    let filled = ((f64::from(width) * fraction).round() as u16).min(width);
    Line::from(vec![
        Span::styled(
            TRACK.repeat(usize::from(filled)),
            Style::default().fg(ACCENT),
        ),
        Span::styled(
            TRACK.repeat(usize::from(width - filled)),
            Style::default().fg(DIM),
        ),
    ])
}

/// A tile has no room for a percentage, so the bar above carries progress and
/// this only has to say when something is finished.
fn caption_meta(item: &Item) -> String {
    let tick = if item.played() { "✓ " } else { "" };
    format!("{tick}{}", rail::meta(item))
}

#[cfg(test)]
#[path = "grid_test.rs"]
mod tests;
