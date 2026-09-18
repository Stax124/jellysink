//! The cover grid: how many tiles fit, which of them are on screen, and the
//! tiles themselves.

use super::{ACCENT, DIM, to_width};
use crate::cover::{self, Covers};
use jellysink_core::jellyfin::model::Item;
use ratatui::Frame;
use ratatui::layout::{Rect, Size};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui_image::FontSize;
use ratatui_image::Image;

const GAP: u16 = 1;
/// The per-tile progress track. An eighth block (`▔`) is a hairline at any font
/// size; an upper half block is four times the height.
const TRACK: &str = "🮂";
/// Under every cover: the shelf rule, the name, and the year/runtime line.
const LABEL_HEIGHT: u16 = 3;

/// Rows of tiles a grid aims to fill the height with. Spending a big monitor's
/// height on bigger covers reads better than five rows of thumbnails.
pub(crate) const TARGET_ROWS: u16 = 2;
/// A Home shelf is one row of tiles in half the body.
pub(crate) const SHELF_ROWS: u16 = 1;
/// The floor under that, which stops a 16:9 still taking a third of a wide
/// screen. A level with fewer items than this spreads over its own count.
const MIN_COLUMNS: u16 = 4;

/// What a grid has to lay out: the rows of tiles it aims to fill the height
/// with, and how many items there are to fill them. Both bound the answer.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Shape {
    pub(crate) target_rows: u16,
    pub(crate) item_count: usize,
}

/// The narrowest a cover may be, whatever the height says. A 2:3 poster stays
/// legible narrow; a 16:9 still does not.
fn minimum_cover_width(aspect: f32) -> u16 {
    if aspect > 1.0 { 24 } else { 16 }
}

/// The tallest a cover may be if `target_rows` of them are to fit, or `None`
/// when the area is too short for that many at the minimum tile width. A single
/// row is capped by the area instead, since a taller tile is not drawn at all.
fn cover_height_budget(
    area: Rect,
    aspect: f32,
    font_size: FontSize,
    target_rows: u16,
) -> Option<u16> {
    let budget = (area.height / target_rows.max(1)).saturating_sub(LABEL_HEIGHT);
    if target_rows == 1 {
        return Some(budget.max(1));
    }
    let floor = cover::rows_for(minimum_cover_width(aspect), aspect, font_size);
    (budget >= floor).then_some(budget)
}

/// What a cover would like to be before the columns are evened out: whatever the
/// height budget affords, bounded by [`MIN_COLUMNS`] and [`minimum_cover_width`].
fn preferred_cover_width(area: Rect, aspect: f32, font_size: FontSize, shape: Shape) -> u16 {
    let minimum = minimum_cover_width(aspect);
    let Some(budget) = cover_height_budget(area, aspect, font_size, shape.target_rows) else {
        return minimum;
    };
    let spread = u16::try_from(shape.item_count)
        .unwrap_or(MIN_COLUMNS)
        .clamp(1, MIN_COLUMNS);
    let widest = (area.width.saturating_sub(GAP * (spread - 1)) / spread).max(minimum);
    let wanted = cover::columns_for(budget, aspect, font_size);
    // A shelf is bound by its height, so widening the tile to the floor would
    // only fit fewer covers at the same size.
    if shape.target_rows == 1 {
        return wanted.min(widest);
    }
    wanted.clamp(minimum, widest)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Metrics {
    pub(crate) columns: usize,
    pub(crate) rows: usize,
    cover: Size,
}

impl Metrics {
    pub(crate) fn cover_size(&self) -> Size {
        self.cover
    }

    pub(crate) fn page(&self) -> usize {
        self.columns * self.rows
    }

    fn tile_height(&self) -> u16 {
        self.cover.height + LABEL_HEIGHT
    }
}

/// Tile geometry for the grid's *inner* area — see [`inner`].
pub(crate) fn metrics(area: Rect, aspect: f32, font_size: FontSize, shape: Shape) -> Metrics {
    let preferred = preferred_cover_width(area, aspect, font_size, shape);
    let columns = (area.width.saturating_add(GAP) / preferred.saturating_add(GAP)).max(1);
    let evened = ((area.width.saturating_sub(GAP * (columns - 1))) / columns).max(1);
    // Evening the columns out can hand a cover more of them than it asked for,
    // and a poster obeying its aspect would grow out of the height budget.
    let max_rows = cover_height_budget(area, aspect, font_size, shape.target_rows)
        .unwrap_or_else(|| cover::rows_for(evened, aspect, font_size));
    let cover = cover::fit(
        Rect::new(0, 0, evened, max_rows),
        aspect,
        font_size,
        max_rows,
    );
    let tile_height = cover.height + LABEL_HEIGHT;
    let columns = usize::from(columns);
    let fits = usize::from((area.height / tile_height).clamp(1, shape.target_rows.max(1)));
    Metrics {
        columns,
        // Rows the grid draws, not rows it could hold: a level too short to
        // fill it must not reserve the height of an empty row.
        rows: fits.min(shape.item_count.div_ceil(columns).max(1)),
        cover: cover.as_size(),
    }
}

/// Scrolls by the least that brings the selection back on screen.
pub(crate) fn scroll_to(offset: usize, selected: usize, metrics: &Metrics) -> usize {
    let row = selected / metrics.columns;
    if row < offset {
        row
    } else if row >= offset + metrics.rows {
        row + 1 - metrics.rows
    } else {
        offset
    }
}

pub(crate) fn inner(area: Rect) -> Rect {
    block(Line::default(), true).inner(area)
}

fn block(title: Line<'_>, focused: bool) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if focused { ACCENT } else { DIM }))
        .border_type(BorderType::Rounded)
        .title(title)
}

/// What a grid draws and where its cursor is. Home draws two of these at once,
/// so neither the rows nor the focus can be read back off the screen.
pub(crate) struct View<'a> {
    pub(crate) items: &'a [Item],
    pub(crate) selected: usize,
    pub(crate) offset: usize,
    /// Rows of tiles to size the covers for.
    pub(crate) rows: u16,
    pub(crate) focused: bool,
}

pub(crate) fn render(
    frame: &mut Frame,
    area: Rect,
    title: Line<'_>,
    view: View<'_>,
    covers: &Covers,
) {
    let items = view.items;
    let block = block(title, view.focused);
    let area_inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(first) = items.first() else {
        frame.render_widget(
            Paragraph::new(Span::styled("nothing here", Style::default().fg(DIM))),
            area_inner,
        );
        return;
    };

    let metrics = metrics(
        area_inner,
        cover::primary_aspect(first),
        covers.font_size(),
        Shape {
            target_rows: view.rows,
            item_count: items.len(),
        },
    );
    let start = view.offset * metrics.columns;
    let top = area_inner.y;
    let stride = metrics.cover.width + GAP;
    // The covers cannot spend what the height budget left over, so the row is
    // centred rather than hanging a whole tile's worth of blank off one side.
    let used = u16::try_from(metrics.columns)
        .unwrap_or(1)
        .saturating_mul(stride)
        .saturating_sub(GAP);
    let left = area_inner.x + area_inner.width.saturating_sub(used) / 2;
    for (index, item) in items.iter().enumerate().skip(start).take(metrics.page()) {
        let slot = index - start;
        let column = u16::try_from(slot % metrics.columns).unwrap_or(0);
        let row = u16::try_from(slot / metrics.columns).unwrap_or(0);
        let tile = Rect {
            x: left + column * stride,
            y: top + row * metrics.tile_height(),
            width: metrics.cover.width,
            height: metrics.tile_height(),
        };
        if tile.bottom() > area_inner.bottom() || tile.right() > area_inner.right() {
            continue;
        }
        let caption = caption_style(index == view.selected, view.focused);
        render_tile(frame, tile, item, caption, covers);
    }
}

/// The caption carries the cursor, so an unfocused shelf still shows where it
/// was left without competing with the shelf that has focus.
fn caption_style(selected: bool, focused: bool) -> Style {
    let style = Style::default().add_modifier(Modifier::BOLD);
    match (selected, focused) {
        (true, true) => style.fg(Color::Black).bg(ACCENT),
        (true, false) => style.fg(Color::Black).bg(DIM),
        (false, _) => style,
    }
}

fn render_tile(frame: &mut Frame, tile: Rect, item: &Item, caption: Style, covers: &Covers) {
    let cover = Rect {
        height: tile.height.saturating_sub(LABEL_HEIGHT),
        ..tile
    };
    if let Some(protocol) = covers
        .key(item, cover.as_size())
        .as_ref()
        .and_then(|key| covers.protocol(key))
    {
        frame.render_widget(Image::new(protocol), cover);
    }

    let rows = [
        watched_rule(item, cover.width),
        Line::from(Span::styled(to_width(&item.label(), cover.width), caption)),
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

/// The shelf rule under a cover, doubling as a partly watched item's progress
/// bar. Selection is carried by the caption's highlight, not by this.
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

/// A tile is only as wide as its cover, so this is the two facts worth that
/// row. Progress is the rule above, and the count says what a tick would.
fn caption_meta(item: &Item) -> String {
    [
        item.unplayed_count().map(|count| format!("{count} left")),
        item.community_rating.map(|rating| format!("★ {rating:.1}")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ")
}

#[cfg(test)]
#[path = "grid_test.rs"]
mod tests;
