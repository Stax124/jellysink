//! The unloaded playlist rows mpv holds ahead of and behind the playing
//! item: how many, and what each one carries.

use crate::runtime::state::Runtime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fill {
    Append,
    /// At position 0. Does not interrupt playback; mpv shifts `playlist-pos`.
    Prepend,
    /// Right after the current item, at the mpv position
    /// `PlaylistWindow::insert_next` returned. Same non-interrupting splice as
    /// `Prepend`, just not pinned to 0.
    Next(usize),
}

impl Runtime {
    /// Splices the pending previous episodes into mpv's playlist. Called once
    /// the current file is loaded, since `loadfile ... replace` would wipe them.
    pub(in crate::runtime) async fn fill_previous_into_mpv(&mut self) {
        let ids = self.window.take_pending_prepend();
        // The whole block at 0 lands in aired order at the front.
        self.load_stub_rows(ids, Fill::Prepend).await;
    }

    /// Appends queue entries past the current mpv window. Titles come from the
    /// series listing; `PlaybackInfo` waits until the item actually plays.
    pub(in crate::runtime) async fn fill_forward_into_mpv(&mut self) {
        let ids: Vec<String> = self.window.forward_ids().to_vec();
        self.load_stub_rows(ids, Fill::Append).await;
    }

    /// Splices `ids` into mpv's playlist right after the current item, at the
    /// mpv position `PlaylistWindow::insert_next` returned. `PlayNext`'s mpv
    /// side: unlike the prepend, there is no upcoming `loadfile ... replace`
    /// to wait out, so this runs immediately instead of being deferred.
    pub(in crate::runtime) async fn insert_next_into_mpv(
        &mut self,
        ids: Vec<String>,
        mpv_pos: usize,
    ) {
        self.load_stub_rows(ids, Fill::Next(mpv_pos)).await;
    }

    /// One `loadlist` of stub rows. No HTTP: the titles are already cached and
    /// the URLs are stubs until the row is actually played.
    async fn load_stub_rows(&mut self, ids: Vec<String>, fill: Fill) {
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
        let refs: Vec<(&str, &str)> = entries
            .iter()
            .map(|(title, url)| (title.as_str(), url.as_str()))
            .collect();
        let Some(mpv) = self.mpv.as_mut() else {
            return;
        };
        let loaded = match fill {
            Fill::Append => mpv.loadlist_append(&refs).await,
            Fill::Prepend => mpv.loadlist_insert_at(&refs, 0).await,
            Fill::Next(index) => mpv.loadlist_insert_at(&refs, index).await,
        };
        if let Err(e) = loaded {
            tracing::warn!(?fill, "playlist fill loadlist: {e:#}");
            return;
        }
        // A prepend or a play-next insert already grew `head`/`tail` when the
        // ids entered the queue, in `PlaylistWindow::prepend` /
        // `PlaylistWindow::insert_next`.
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

/// `(title, url)` for one playlist row. `token` is `Some` only when the
/// Authorization header is not covering mpv, since mpv persists playlist
/// entries to watch_later files; the title fallback never carries it at all.
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
