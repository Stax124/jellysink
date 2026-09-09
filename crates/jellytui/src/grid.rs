//! The cover grid: how many tiles fit, which of them are on screen, and the
//! tiles themselves.

use super::cover::{self, CoverKey, Covers};
use super::rail;
use super::ui::{ACCENT, DIM, to_width};
use jellysink_core::jellyfin::model::Item;
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

/// Rows of tiles a grid aims to fill the height with. Height is what a big
/// monitor has most of, so spending it on bigger covers and longer captions
/// reads better than stacking five rows of thumbnails.
const TARGET_ROWS: u16 = 2;
/// The floor under that: a tile is never sized so generously that a row holds
/// fewer than this, which is what stops a 16:9 still from taking a third of a
/// wide screen on its own.
const MIN_COLUMNS: u16 = 4;

/// The narrowest a tile may be, whatever the height says. A 2:3 poster stays
/// legible narrow; a 16:9 still does not.
fn minimum_tile_width(aspect: f32) -> u16 {
    if aspect > 1.0 { 26 } else { 18 }
}

/// The tallest a cover may be if [`TARGET_ROWS`] of them are to fit, or `None`
/// when the area is too short for that many at the minimum tile width. Capping
/// a cover that was never going to reach two rows would only shrink it, so a
/// short terminal keeps the tiles it has.
fn cover_height_budget(area: Rect, aspect: f32, font_size: FontSize) -> Option<u16> {
    let budget = (area.height / TARGET_ROWS).saturating_sub(LABEL_HEIGHT);
    let floor = cover::rows_for(minimum_tile_width(aspect) - 2, aspect, font_size);
    (budget >= floor).then_some(budget)
}

/// What a tile would like to be before the columns are evened out across the
/// area: whatever the height budget affords, bounded by both [`MIN_COLUMNS`]
/// and [`minimum_tile_width`].
fn preferred_tile_width(area: Rect, aspect: f32, font_size: FontSize) -> u16 {
    let minimum = minimum_tile_width(aspect);
    let Some(budget) = cover_height_budget(area, aspect, font_size) else {
        return minimum;
    };
    let widest = (area.width.saturating_sub(GAP * (MIN_COLUMNS - 1)) / MIN_COLUMNS).max(minimum);
    (cover::columns_for(budget, aspect, font_size) + 2).clamp(minimum, widest)
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
    let preferred = preferred_tile_width(area, aspect, font_size);
    let columns = (area.width.saturating_add(GAP) / preferred.saturating_add(GAP)).max(1);
    let tile_width = ((area.width.saturating_sub(GAP * (columns - 1))) / columns).max(1);
    // Evening the tiles out across the width can hand a tile more columns than
    // it asked for, and a poster obeying its aspect would grow out of the
    // height budget with them — so the cover is fitted to both.
    let cover_width = tile_width.saturating_sub(2).max(1);
    let max_rows = cover_height_budget(area, aspect, font_size)
        .unwrap_or_else(|| cover::rows_for(cover_width, aspect, font_size));
    let cover = cover::fit(
        Rect::new(0, 0, cover_width, max_rows),
        aspect,
        font_size,
        max_rows,
    );
    let tile_height = cover.height + LABEL_HEIGHT;
    Metrics {
        columns: usize::from(columns),
        rows: usize::from((area.height / tile_height).max(1)),
        tile: Size::new(tile_width, tile_height),
        cover: cover.as_size(),
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
    // A cover can come out narrower than the tile it sits in, because the
    // height budget bounds it before the width does. Centring it centres the
    // caption with it, since both are drawn against this rect.
    let cover = Rect {
        x: tile.x + 1 + (tile.width.saturating_sub(2 + metrics.cover.width)) / 2,
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
