//! Covers kept between sessions: the bytes the server sent, not the protocol
//! they were encoded into. A `Protocol` belongs to one rect at one cell size
//! and is worthless the moment either moves; these bytes are the round trip,
//! which is the part worth not paying twice.
//!
//! Nothing here is allowed to fail a cover. Every path degrades to a miss, and
//! a miss is a fetch.

use super::CoverKey;
use jellysink_core::config::atomic_write;
use jellysink_core::jellyfin::browse::bucket_pixels;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

/// How much may be written between two prunes, as a fraction of the budget. A
/// scan is a stat per entry, so it is worth amortising, and a quarter over
/// budget is a few tens of megabytes rather than a full library.
const PRUNE_EVERY: u64 = 4;

/// How far under budget a prune goes. Stopping exactly at the budget would
/// leave the next cover to prune again.
const PRUNE_TO: f64 = 0.9;

/// A handle onto the cover directory, cloned into the task that fetches a
/// cover so the read, the write and the decode all happen on the one blocking
/// thread.
#[derive(Clone)]
pub(crate) struct CoverDisk {
    dir: PathBuf,
    budget: u64,
    written: Arc<AtomicU64>,
}

impl CoverDisk {
    pub(crate) fn new(dir: PathBuf, budget_mb: u64) -> Self {
        Self {
            dir,
            budget: budget_mb.saturating_mul(1024 * 1024),
            written: Arc::new(AtomicU64::new(0)),
        }
    }

    /// For the tests that are not about the disk. A real run reaches the same
    /// state through `cover_cache_mb = 0`.
    #[cfg(test)]
    pub(crate) fn disabled() -> Self {
        Self::new(PathBuf::new(), 0)
    }

    fn enabled(&self) -> bool {
        self.budget > 0
    }

    /// Keyed on the *bucketed* pixels rather than the box, so the sizes either
    /// side of a 64 px boundary share the file they also share a request for.
    /// Ids and tags are hex, so the key can be its own filename.
    fn path(&self, key: &CoverKey) -> PathBuf {
        let (box_, cell) = (key.size, key.cell);
        let width = bucket_pixels(u32::from(box_.width) * u32::from(cell.width));
        let height = bucket_pixels(u32::from(box_.height) * u32::from(cell.height));
        self.dir.join(format!(
            "{}-{}-{width}x{height}.img",
            sanitize(&key.item_id),
            sanitize(&key.image_tag)
        ))
    }

    pub(crate) fn read(&self, key: &CoverKey) -> Option<Vec<u8>> {
        if !self.enabled() {
            return None;
        }
        let path = self.path(key);
        let bytes = fs::read(&path).ok()?;
        touch(&path);
        Some(bytes)
    }

    pub(crate) fn write(&self, key: &CoverKey, bytes: &[u8]) {
        if !self.enabled() {
            return;
        }
        if let Err(err) = fs::create_dir_all(&self.dir)
            .map_err(color_eyre::Report::from)
            .and_then(|()| atomic_write(&self.path(key), bytes, 0o644))
        {
            tracing::debug!(%err, "cover not cached");
            return;
        }
        let before = self
            .written
            .fetch_add(bytes.len() as u64, Ordering::Relaxed);
        if before + bytes.len() as u64 >= self.budget / PRUNE_EVERY {
            self.prune();
        }
    }

    /// A file that will not decode is worth one removal rather than a decode
    /// attempt per scroll.
    pub(crate) fn discard(&self, key: &CoverKey) {
        if self.enabled() {
            let _ = fs::remove_file(self.path(key));
        }
    }

    /// Drops the least recently read covers until the directory is back under
    /// budget. Also what trims a budget the user has just lowered, which is why
    /// it runs once at startup as well.
    pub(crate) fn prune(&self) {
        if !self.enabled() {
            return;
        }
        self.written.store(0, Ordering::Relaxed);
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return;
        };
        let mut covers: Vec<(PathBuf, u64, SystemTime)> = entries
            .flatten()
            .filter_map(|entry| {
                let meta = entry.metadata().ok()?;
                let modified = meta.modified().ok()?;
                meta.is_file().then(|| (entry.path(), meta.len(), modified))
            })
            .collect();
        let mut total: u64 = covers.iter().map(|(_, len, _)| len).sum();
        if total <= self.budget {
            return;
        }

        covers.sort_by_key(|&(_, _, modified)| modified);
        let target = (self.budget as f64 * PRUNE_TO) as u64;
        let mut dropped = 0usize;
        for (path, len, _) in covers {
            if total <= target {
                break;
            }
            if fs::remove_file(&path).is_ok() {
                total -= len;
                dropped += 1;
            }
        }
        tracing::debug!(dropped, remaining_bytes = total, "cover cache pruned");
    }
}

/// A read is what makes a cover recent, so eviction keeps what is being looked
/// at rather than what was fetched last. Best-effort: an unbumped mtime costs
/// the entry its place in the queue and nothing else.
fn touch(path: &Path) {
    if let Ok(file) = fs::File::open(path) {
        let _ = file.set_modified(SystemTime::now());
    }
}

fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

#[cfg(test)]
#[path = "disk_test.rs"]
mod tests;
