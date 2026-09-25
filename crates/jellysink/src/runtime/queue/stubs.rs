//! The unloaded playlist rows mpv holds ahead of and behind the playing
//! item: how many, and what each one carries.

use crate::runtime::state::Runtime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::runtime) enum Fill {
    Append,
    /// Does not interrupt playback; mpv shifts `playlist-pos`.
    InsertAt(usize),
}

impl Runtime {
    /// Splices the pending previous episodes into mpv's playlist. Called once
    /// the current file is loaded, since `loadfile ... replace` would wipe them.
    pub(in crate::runtime) async fn fill_previous_into_mpv(&mut self) {
        let ids = self.window.take_pending_prepend();
        // The whole block at 0 lands in aired order at the front.
        self.load_stub_rows(ids, Fill::InsertAt(0)).await;
    }

    /// Appends queue entries past the current mpv window. Titles come from the
    /// series listing; `PlaybackInfo` waits until the item actually plays.
    pub(in crate::runtime) async fn fill_forward_into_mpv(&mut self) {
        let ids: Vec<String> = self.window.forward_ids().to_vec();
        self.load_stub_rows(ids, Fill::Append).await;
    }

    /// One `loadlist` of stub rows. No HTTP: the titles are already cached and
    /// the URLs are stubs until the row is actually played.
    pub(in crate::runtime) async fn load_stub_rows(&mut self, ids: Vec<String>, fill: Fill) {
        if ids.is_empty() || self.mpv.is_none() {
            return;
        }
        let n = ids.len();
        tracing::debug!(
            n,
            ?fill,
            origin = self.window.origin(),
            head = self.window.head(),
            tail = self.window.tail(),
            "filling mpv playlist"
        );
        let entries = self.playlist_stub_entries(&ids);
        let Some(mpv) = self.mpv.as_mut() else {
            return;
        };
        let index = match fill {
            Fill::Append => None,
            Fill::InsertAt(index) => Some(index),
        };
        if let Err(e) = mpv.loadlist(&entries, index).await {
            tracing::warn!(?fill, "playlist fill loadlist: {e:#}");
            return;
        }
        // An insert already grew `head`/`tail` when the ids entered the queue.
        if fill == Fill::Append {
            self.window.note_appended(n);
        }
        tracing::debug!(n, ?fill, tail = self.window.tail(), "filled mpv playlist");
    }

    fn playlist_stub_entries(&self, ids: &[String]) -> Vec<(String, String)> {
        let token = (!self.mpv_auth_header_set).then_some(self.api.token.as_str());
        ids.iter()
            .map(|id| {
                playlist_stub_entry(
                    &self.api.server,
                    id,
                    self.titles.get(id).map(String::as_str),
                    token,
                )
            })
            .collect()
    }
}

/// `(title, url)` for one playlist row. `token` is `Some` only when the header
/// is not covering mpv, which persists these rows to watch_later files.
fn playlist_stub_entry(
    server: &str,
    id: &str,
    title: Option<&str>,
    token: Option<&str>,
) -> (String, String) {
    let url = jellysink_core::jellyfin::url::direct_stream_url(server, id, id, None, token);
    let title = title.map(str::to_string).unwrap_or_else(|| {
        jellysink_core::jellyfin::url::direct_stream_url(server, id, id, None, None)
    });
    (title, url)
}

#[cfg(test)]
#[path = "stubs_test.rs"]
mod tests;
