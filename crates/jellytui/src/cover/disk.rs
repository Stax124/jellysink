//! Covers kept between sessions: the bytes the server sent, not the protocol
//! they were encoded into, which belongs to one rect at one cell size. Nothing
//! here may fail a cover — every path degrades to a miss, and a miss is a fetch.

use super::CoverKey;
use jellysink_core::config::atomic_write;
use jellysink_core::jellyfin::browse::bucket_pixels;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

/// How much may be written between two prunes, as a fraction of the budget. A
/// scan is a stat per entry, so it is worth amortising.
const PRUNE_AFTER: f64 = 0.25;

/// How far under budget a prune goes. Stopping exactly at the budget would
/// leave the next cover to prune again.
const PRUNE_TO: f64 = 0.9;

/// A handle onto the cover directory, cloned into the task that fetches a cover
/// so the read, the write and the decode share one blocking thread.
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
    fn path(&self, key: &CoverKey) -> PathBuf {
        let width = bucket_pixels(u32::from(key.size.width) * u32::from(key.cell.width));
        let height = bucket_pixels(u32::from(key.size.height) * u32::from(key.cell.height));
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
        if before + bytes.len() as u64 >= (self.budget as f64 * PRUNE_AFTER) as u64 {
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
    /// budget, including a budget the user has just lowered.
    pub(crate) fn prune(&self) {
        if !self.enabled() {
            return;
        }
        self.written.store(0, Ordering::Relaxed);
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            // Not yet written to is the ordinary first run; anything else
            // leaves the cache growing with nothing to say why.
            Err(err) => {
                if err.kind() != std::io::ErrorKind::NotFound {
                    tracing::debug!(%err, "cover cache not pruned");
                }
                return;
            }
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
/// at rather than what was fetched last. Best-effort by design.
fn touch(path: &Path) {
    if let Ok(file) = fs::File::open(path) {
        let _ = file.set_modified(SystemTime::now());
    }
}

/// The id and the tag come from the server, so a `/` or a `..` in one would
/// make the key a path rather than a name.
fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

#[cfg(test)]
#[path = "disk_test.rs"]
mod tests;
