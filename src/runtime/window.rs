use crate::mpv::EndFileReason;
use serde_json::{Value, json};
use std::sync::Arc;

/// The queue, plus how much of it mpv currently holds. See `specs/playlist.md`.
///
/// mpv's playlist is always a contiguous slice of the queue:
///
/// ```text
/// mpv playlist == queue.items[origin .. origin + head + 1 + tail]
/// ```
///
/// The `+ 1` is the current item, whose mpv position is therefore
/// `queue.index - origin` ([`Self::expected_pos`]) — not `head`, which only
/// coincides with it until a playlist jump moves `index`. A prepend leaves
/// `origin` alone, since the index and the mpv position shift together.
#[derive(Debug, Default)]
pub(super) struct PlaylistWindow {
    queue: Queue,
    /// Queue index at mpv playlist position 0.
    origin: usize,
    /// Queue entries already in mpv *before* the current one.
    head: usize,
    /// Queue entries already in mpv *after* the current one.
    tail: usize,
    /// Previous episodes spliced into the queue but not yet into mpv; they wait
    /// out the `loadfile ... replace` that would wipe them.
    pending_prepend: Vec<String>,
    /// The rendered `NowPlayingQueue` payload, rebuilt only when the queue
    /// changes; it goes out once a second and can be 500 entries long.
    now_playing: Arc<Vec<Value>>,
}

impl PlaylistWindow {
    /// `start_current`: mpv is about to hold the current item and nothing else.
    pub(super) fn reset_to_current(&mut self) {
        self.origin = self.queue.index;
        self.head = 0;
        self.tail = 0;
        self.pending_prepend.clear();
    }

    /// `stop_playback`: mpv holds nothing. `origin` is left alone; the next
    /// `reset_to_current` sets it.
    pub(super) fn clear(&mut self) {
        self.head = 0;
        self.tail = 0;
        self.pending_prepend.clear();
    }

    pub(super) fn origin(&self) -> usize {
        self.origin
    }

    pub(super) fn head(&self) -> usize {
        self.head
    }

    pub(super) fn tail(&self) -> usize {
        self.tail
    }

    /// The current item's position in mpv's playlist. Deriving it from `head`
    /// instead goes stale on a playlist jump and misreads every later EOF.
    pub(super) fn expected_pos(&self) -> usize {
        self.queue.index.saturating_sub(self.origin)
    }

    /// The queue index mpv playlist position `playlist_pos` refers to, if it is
    /// inside the queue.
    pub(super) fn queue_index_at(&self, playlist_pos: usize) -> Option<usize> {
        queue_index_at(self.origin, playlist_pos, self.queue.items.len())
    }

    /// Queue entries past everything mpv already holds.
    pub(super) fn forward_ids(&self) -> &[String] {
        self.queue
            .items
            .get(self.origin + self.head + 1 + self.tail..)
            .unwrap_or(&[])
    }

    /// Records that `n` entries were appended to the end of mpv's playlist.
    pub(super) fn note_appended(&mut self, n: usize) {
        self.tail += n;
    }

    /// Splices previous episodes into the queue ahead of the current item and
    /// holds them for [`Self::take_pending_prepend`]. Returns how many.
    pub(super) fn prepend(&mut self, previous: Vec<String>) -> usize {
        let n = self.queue.insert_before_current(previous.clone());
        self.rebuild_now_playing();
        // Queue index and mpv position shift together, so only `head` grows.
        self.head += n;
        self.pending_prepend = previous;
        n
    }

    // --- Queue delegation ---------------------------------------------------
    // `Queue`'s fields stay private: `index` and `origin`/`head`/`tail` are one
    // invariant.

    pub(super) fn items(&self) -> &[String] {
        &self.queue.items
    }

    pub(super) fn len(&self) -> usize {
        self.queue.items.len()
    }

    pub(super) fn index(&self) -> usize {
        self.queue.index
    }

    pub(super) fn current(&self) -> Option<&str> {
        self.queue.current()
    }

    pub(super) fn peek_next(&self) -> Option<&str> {
        self.queue.peek_next()
    }

    pub(super) fn has_next(&self) -> bool {
        self.queue.has_next()
    }

    pub(super) fn advance(&mut self) -> Option<&str> {
        self.queue.advance()
    }

    pub(super) fn previous(&mut self) -> Option<&str> {
        self.queue.previous()
    }

    pub(super) fn replace(&mut self, items: Vec<String>, start_index: usize) {
        self.queue.replace(items, start_index);
        self.rebuild_now_playing();
    }

    pub(super) fn append(&mut self, ids: Vec<String>) {
        self.queue.append(ids);
        self.rebuild_now_playing();
    }

    pub(super) fn insert_next(&mut self, ids: Vec<String>) {
        self.queue.insert_next(ids);
        self.rebuild_now_playing();
    }

    /// The `NowPlayingQueue` Jellyfin's now-playing view renders.
    pub(super) fn now_playing_queue(&self) -> Arc<Vec<Value>> {
        Arc::clone(&self.now_playing)
    }

    /// Eager on mutation rather than lazy per report: mutations are a handful
    /// per play, reports are one a second.
    fn rebuild_now_playing(&mut self) {
        self.now_playing = Arc::new(
            self.queue
                .items
                .iter()
                .enumerate()
                .map(|(i, id)| json!({"Id": id, "PlaylistItemId": format!("playlistItem{i}")}))
                .collect(),
        );
    }

    /// Moves the current item to `index` after mpv jumped there on its own.
    /// `origin` is untouched: the window itself did not move.
    pub(super) fn adopt_index(&mut self, index: usize) {
        self.queue.index = index;
    }

    /// Takes the episodes waiting to be spliced into mpv, leaving none.
    pub(super) fn take_pending_prepend(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending_prepend)
    }

    /// The window invariant spelled out, for the tests to assert against.
    #[cfg(test)]
    fn mpv_playlist(&self) -> &[String] {
        let end = (self.origin + self.head + 1 + self.tail).min(self.queue.items.len());
        self.queue.items.get(self.origin..end).unwrap_or(&[])
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Queue {
    items: Vec<String>,
    index: usize,
}

impl Queue {
    pub(super) fn current(&self) -> Option<&str> {
        self.items.get(self.index).map(String::as_str)
    }

    pub(super) fn replace(&mut self, items: Vec<String>, start_index: usize) {
        self.items = items;
        self.index = start_index.min(self.items.len().saturating_sub(1));
        if self.items.is_empty() {
            self.index = 0;
        }
    }

    pub(super) fn insert_next(&mut self, ids: Vec<String>) {
        let at = self.index.saturating_add(1).min(self.items.len());
        self.items.splice(at..at, ids);
    }

    pub(super) fn append(&mut self, ids: Vec<String>) {
        self.items.extend(ids);
    }

    /// Splices `ids` in immediately before the current item, keeping `index`
    /// on the same item. Returns how many were inserted. At `index`, not 0, so
    /// they land inside mpv's window rather than in a hole before `origin`.
    pub(super) fn insert_before_current(&mut self, ids: Vec<String>) -> usize {
        let n = ids.len();
        let at = self.index;
        self.items.splice(at..at, ids);
        self.index += n;
        n
    }

    pub(super) fn advance(&mut self) -> Option<&str> {
        if self.index + 1 < self.items.len() {
            self.index += 1;
            self.current()
        } else {
            None
        }
    }

    pub(super) fn previous(&mut self) -> Option<&str> {
        if self.index > 0 {
            self.index -= 1;
            self.current()
        } else {
            self.current()
        }
    }

    pub(super) fn has_next(&self) -> bool {
        !self.items.is_empty() && self.index + 1 < self.items.len()
    }

    pub(super) fn peek_next(&self) -> Option<&str> {
        self.items.get(self.index + 1).map(String::as_str)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EndFileAction {
    Ignore,
    Advance,
    Stop,
}

pub(super) fn end_file_action(
    transitioning: bool,
    stopping: bool,
    reason: EndFileReason,
) -> EndFileAction {
    if transitioning || stopping {
        return EndFileAction::Ignore;
    }
    match reason {
        // Advance may still expand the series; stopping is
        // play_next_or_stop's call.
        EndFileReason::Eof | EndFileReason::Redirect => EndFileAction::Advance,
        EndFileReason::Quit | EndFileReason::Stop | EndFileReason::Error => EndFileAction::Stop,
        EndFileReason::Other => EndFileAction::Ignore,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PlaylistEof {
    NextInMpv,
    NextNotInMpv,
    WaitForMpv,
    Stop,
}

/// `playlist-pos` p corresponds to `queue.index = origin + p`.
pub(super) fn queue_index_at(
    origin: usize,
    playlist_pos: usize,
    queue_len: usize,
) -> Option<usize> {
    origin.checked_add(playlist_pos).filter(|i| *i < queue_len)
}

/// After EOF (caller already applied `end_file_action`). `expected_pos` is the
/// playlist index of the file that just ended; `from_eof` separates mpv's own
/// `end-file` from a user Next, because `keep-open=yes` already auto-plays the
/// next entry and a `playlist-next` on top of it would skip to N+2.
pub(super) fn playlist_eof(
    playlist_pos: usize,
    playlist_count: usize,
    queue_has_next: bool,
    expected_pos: usize,
    from_eof: bool,
) -> PlaylistEof {
    if playlist_pos > expected_pos {
        PlaylistEof::WaitForMpv
    } else if playlist_pos + 1 < playlist_count {
        if from_eof {
            PlaylistEof::WaitForMpv
        } else {
            PlaylistEof::NextInMpv
        }
    } else if queue_has_next {
        PlaylistEof::NextNotInMpv
    } else {
        PlaylistEof::Stop
    }
}

/// `playlist-next` / OSC jump ends the old file with `stop`. That is not a user Stop.
pub(super) fn ignore_stop_for_playlist(reason: EndFileReason, playlist_count: usize) -> bool {
    reason == EndFileReason::Stop && playlist_count > 1
}

#[cfg(test)]
#[path = "window_test.rs"]
mod tests;
