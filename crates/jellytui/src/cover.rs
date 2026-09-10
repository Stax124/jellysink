//! Cover art: which image an item wants, and the bounded cache of decoded
//! terminal protocols behind it.

use color_eyre::eyre::{Result, WrapErr};
use jellysink_core::jellyfin::auth::Api;
use jellysink_core::jellyfin::model::Item;
use ratatui::backend::WindowSize;
use ratatui::layout::{Rect, Size};
use ratatui_image::FontSize;
use ratatui_image::Resize;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;
use std::collections::{HashMap, HashSet, VecDeque};

/// Decoded covers held at once. A grid shows around fifteen, so this carries a
/// few screens of scrollback without the encoded frames adding up.
const CACHE_CAPACITY: usize = 64;

/// How far a measured cell has to be from the one the covers are encoded with
/// before it counts as another display rather than as the window's padding,
/// which is in its pixel size but not in a cell. A scale factor is at least a
/// quarter away, so this only has to clear the padding.
const NEW_GRID_THRESHOLD: f32 = 0.05;

/// Which image to draw, how large, and against which pixel grid. Both sizes
/// belong to the identity because a protocol is encoded against one rect at one
/// cell size — after a resize, or a move to a display of another scale, the old
/// encoding is the wrong one rather than a stale one.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct CoverKey {
    item_id: String,
    image_tag: String,
    size: Size,
    cell: Size,
}

pub(super) struct Covers {
    picker: Picker,
    ready: HashMap<CoverKey, Protocol>,
    order: VecDeque<CoverKey>,
    in_flight: HashSet<CoverKey>,
    /// Items the server has no artwork for, so revisiting the row does not ask
    /// again. A request that merely failed is not in here — that one is worth
    /// retrying next time the item is looked at.
    absent: HashSet<CoverKey>,
}

impl Covers {
    pub(super) fn new(picker: Picker) -> Self {
        Self {
            picker,
            ready: HashMap::new(),
            order: VecDeque::new(),
            in_flight: HashSet::new(),
            absent: HashSet::new(),
        }
    }

    pub(super) fn key(&self, item: &Item, size: Size) -> Option<CoverKey> {
        let font_size = self.picker.font_size();
        Some(CoverKey {
            item_id: item.id.clone(),
            image_tag: item.primary_image_tag()?.to_string(),
            size,
            cell: Size::new(font_size.width, font_size.height),
        })
    }

    /// Takes the terminal's current pixels per cell, and encodes against that
    /// grid from here on. Everything encoded against the previous one goes: a
    /// key carries the cell, so those entries could never be looked up again,
    /// and the requests still in flight land under the old key rather than on
    /// screen.
    pub(super) fn set_cell_size(&mut self, cell: Option<Size>) {
        let Some(cell) = cell else { return };
        if !self.is_new_grid(cell) {
            return;
        }
        self.picker = repicker(&self.picker, cell);
        self.ready.clear();
        self.order.clear();
        self.absent.clear();
    }

    fn is_new_grid(&self, cell: Size) -> bool {
        // One axis is enough: a display's scale is uniform, and the other
        // would differ only in its rounding.
        let font_size = self.picker.font_size();
        let ratio = f32::from(cell.width) / f32::from(font_size.width);
        (ratio - 1.0).abs() >= NEW_GRID_THRESHOLD
    }

    pub(super) fn protocol(&self, key: &CoverKey) -> Option<&Protocol> {
        self.ready.get(key)
    }

    /// Whether the caller should start a request for `key`, marking it in
    /// flight if so.
    pub(super) fn claim(&mut self, key: &CoverKey) -> bool {
        if self.ready.contains_key(key) || self.in_flight.contains(key) || self.absent.contains(key)
        {
            return false;
        }
        self.in_flight.insert(key.clone());
        true
    }

    /// `None` records that the server has no such image. A request that failed
    /// for any other reason goes through [`Covers::release`] instead.
    pub(super) fn store(&mut self, key: CoverKey, protocol: Option<Protocol>) {
        self.in_flight.remove(&key);
        let Some(protocol) = protocol else {
            self.absent.insert(key);
            return;
        };
        if self.ready.insert(key.clone(), protocol).is_none() {
            self.order.push_back(key);
        }
        while self.order.len() > CACHE_CAPACITY {
            if let Some(oldest) = self.order.pop_front() {
                self.ready.remove(&oldest);
            }
        }
    }

    /// Gives up on a request without concluding anything about the item, so a
    /// blip does not cost the cover for the rest of the session.
    pub(super) fn release(&mut self, key: &CoverKey) {
        self.in_flight.remove(key);
    }

    pub(super) fn picker(&self) -> Picker {
        self.picker.clone()
    }

    pub(super) fn font_size(&self) -> FontSize {
        self.picker.font_size()
    }
}

/// The picker again at `cell` pixels per cell, keeping the protocol the startup
/// query settled on. Deprecated in favour of that query, which cannot be run a
/// second time with the alternate screen up — and 11.x has no way to hand a new
/// font size to the picker it already returned.
fn repicker(picker: &Picker, cell: Size) -> Picker {
    #[allow(deprecated)]
    let mut rebuilt = Picker::from_fontsize(FontSize::new(cell.width, cell.height));
    rebuilt.set_protocol_type(picker.protocol_type());
    rebuilt
}

/// The shape of an item's primary image, as width ÷ height. The server's own
/// measurement wins where it sent one: a library's primary image is a 16:9
/// banner, and a box reserved for the 2:3 poster its kind suggests is drawn as
/// a gap between the picture and the text under it. Where it sent none, an
/// episode has a 16:9 still and everything else a poster.
pub(super) fn primary_aspect(item: &Item) -> f32 {
    item.primary_image_aspect_ratio
        .map(|ratio| ratio as f32)
        .filter(|ratio| ratio.is_finite() && *ratio > 0.0)
        .unwrap_or(if item.kind() == "Episode" {
            16.0 / 9.0
        } else {
            2.0 / 3.0
        })
}

/// How many rows a cover `width` cells wide needs to hold `aspect`. A cell is
/// about twice as tall as it is wide, so this is never the same number.
pub(super) fn rows_for(width: u16, aspect: f32, font_size: FontSize) -> u16 {
    let pixels = f32::from(width) * f32::from(font_size.width) / aspect;
    ((pixels / f32::from(font_size.height)).ceil() as u16).max(1)
}

/// The inverse of [`rows_for`]: how wide a cover `rows` tall comes out.
pub(super) fn columns_for(rows: u16, aspect: f32, font_size: FontSize) -> u16 {
    let pixels = f32::from(rows) * f32::from(font_size.height) * aspect;
    ((pixels / f32::from(font_size.width)).floor() as u16).max(1)
}

/// The largest box of `aspect` (width ÷ height) that fits inside both `area`
/// and `max_rows`, centred horizontally.
pub(super) fn fit(area: Rect, aspect: f32, font_size: FontSize, max_rows: u16) -> Rect {
    let max_rows = max_rows.clamp(1, area.height.max(1));
    let rows_at_full_width = rows_for(area.width, aspect, font_size);
    let (width, height) = if rows_at_full_width <= max_rows {
        (area.width, rows_at_full_width)
    } else {
        let width = columns_for(max_rows, aspect, font_size);
        (width.clamp(1, area.width.max(1)), max_rows)
    };
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y,
        width,
        height,
    }
}

/// The terminal's pixels per cell, worked out from the size the tty reports for
/// the window. `None` where it leaves the pixel fields at zero — tmux and a
/// plain xterm do, and there is nothing to measure against then.
pub(super) fn cell_size(window: WindowSize) -> Option<Size> {
    let (grid, pixels) = (window.columns_rows, window.pixels);
    if grid.width == 0 || grid.height == 0 || pixels.width == 0 || pixels.height == 0 {
        return None;
    }
    Some(Size::new(
        pixels.width / grid.width,
        pixels.height / grid.height,
    ))
}

/// Queries the terminal for its graphics protocol and cell size. Has to run
/// before the alternate screen is taken: the query goes out on stdout and the
/// answer comes back on stdin. Halfblocks are the fallback rather than a
/// failure — tmux and a plain xterm land there and still get a picture.
pub(super) fn detect_picker() -> Picker {
    Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks())
}

/// One cover, resized by the server rather than downloaded whole — the same
/// lesson `specs/tui.md` records about `/Sessions`, at a smaller scale.
/// `Ok(None)` for an item the server has no artwork for. Decode and encode are
/// real CPU work on the thread that draws, so they go to `spawn_blocking`.
pub(super) async fn fetch(api: &Api, picker: Picker, key: &CoverKey) -> Result<Option<Protocol>> {
    let started = std::time::Instant::now();
    let (font_size, size) = (picker.font_size(), key.size);
    let width = u32::from(size.width) * u32::from(font_size.width);
    let height = u32::from(size.height) * u32::from(font_size.height);
    let Some(bytes) = api
        .primary_image(&key.item_id, &key.image_tag, width, height)
        .await?
    else {
        return Ok(None);
    };
    let (len, fetched_ms) = (bytes.len(), started.elapsed().as_millis());
    let protocol = tokio::task::spawn_blocking(move || {
        let image = image::load_from_memory(&bytes).wrap_err("decoding cover")?;
        picker
            .new_protocol(image, size, Resize::Fit(None))
            .wrap_err("encoding cover for the terminal")
            .map(Some)
    })
    .await
    .wrap_err("cover worker")?;
    tracing::trace!(
        item_id = %key.item_id,
        bytes = len,
        fetched_ms,
        total_ms = started.elapsed().as_millis(),
        "cover"
    );
    protocol
}

#[cfg(test)]
#[path = "cover_test.rs"]
mod tests;
