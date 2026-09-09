//! Cover art: which image an item wants, and the bounded cache of decoded
//! terminal protocols behind it.

use color_eyre::eyre::{Result, WrapErr};
use jellysink_core::jellyfin::auth::Api;
use jellysink_core::jellyfin::model::Item;
use ratatui::layout::{Rect, Size};
use ratatui_image::FontSize;
use ratatui_image::Resize;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::Protocol;
use std::collections::{HashMap, HashSet, VecDeque};

/// Decoded covers held at once. A grid shows around fifteen, so this carries a
/// few screens of scrollback without the encoded frames adding up.
const CACHE_CAPACITY: usize = 64;

/// Which image to draw, and how large. The size belongs to the identity
/// because a protocol is encoded against one rect — after a terminal resize
/// the old encoding is the wrong one rather than a stale one.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct CoverKey {
    item_id: String,
    image_tag: String,
    size: Size,
}

impl CoverKey {
    /// The item's own primary image: a 2:3 poster for a series, season or
    /// movie, a 16:9 still for an episode.
    pub(super) fn primary(item: &Item, size: Size) -> Option<Self> {
        Some(Self {
            item_id: item.id.clone(),
            image_tag: item.primary_image_tag()?.to_string(),
            size,
        })
    }
}

pub(super) struct Covers {
    picker: Picker,
    scale: f32,
    ready: HashMap<CoverKey, Protocol>,
    order: VecDeque<CoverKey>,
    in_flight: HashSet<CoverKey>,
    /// Items the server has no artwork for, so revisiting the row does not ask
    /// again. A request that merely failed is not in here — that one is worth
    /// retrying next time the item is looked at.
    absent: HashSet<CoverKey>,
}

impl Covers {
    pub(super) fn new(picker: Picker, image_scale: f32) -> Self {
        // Halfblocks are ordinary cells, so there is no pixel grid to be out
        // of step with — and an over-encoded halfblocks image is cropped to
        // the area rather than drawn sharper.
        let scale = if picker.protocol_type() == ProtocolType::Halfblocks {
            1.0
        } else {
            image_scale
        };
        Self {
            picker,
            scale,
            ready: HashMap::new(),
            order: VecDeque::new(),
            in_flight: HashSet::new(),
            absent: HashSet::new(),
        }
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

    pub(super) fn scale(&self) -> f32 {
        self.scale
    }
}

/// The cell box an image is fetched and encoded for, which is not always the
/// box it is drawn in. Kitty sizes a placement by dividing the image's pixels
/// by the terminal's *real* cell size, while `ratatui-image` lays out the
/// placeholder cells using the size the terminal *reports* — and on a HiDPI
/// display those differ by the display's scale factor, so a cover encoded for
/// the reported grid covers a fraction of the box we reserved for it. Asking
/// for `scale` times the pixels puts the factor back; the widget clamps the
/// cells it draws to its area, so the surplus costs pixels, not layout.
fn encoded_size(size: Size, scale: f32) -> Size {
    let grow = |cells: u16| ((f32::from(cells) * scale).round() as u16).max(1);
    Size::new(grow(size.width), grow(size.height))
}

/// The shape of an item's primary image, as width ÷ height. Jellyfin gives an
/// episode a 16:9 still and everything else a 2:3 poster.
pub(super) fn primary_aspect(item: &Item) -> f32 {
    if item.kind() == "Episode" {
        16.0 / 9.0
    } else {
        2.0 / 3.0
    }
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
pub(super) async fn fetch(
    api: &Api,
    picker: Picker,
    scale: f32,
    key: &CoverKey,
) -> Result<Option<Protocol>> {
    let font_size = picker.font_size();
    let size = encoded_size(key.size, scale);
    let width = u32::from(size.width) * u32::from(font_size.width);
    let height = u32::from(size.height) * u32::from(font_size.height);
    let Some(bytes) = api
        .primary_image(&key.item_id, &key.image_tag, width, height)
        .await?
    else {
        return Ok(None);
    };
    tokio::task::spawn_blocking(move || {
        let image = image::load_from_memory(&bytes).wrap_err("decoding cover")?;
        picker
            .new_protocol(image, size, Resize::Fit(None))
            .wrap_err("encoding cover for the terminal")
            .map(Some)
    })
    .await
    .wrap_err("cover worker")?
}

#[cfg(test)]
#[path = "cover_test.rs"]
mod tests;
