//! The play mpv is loading, and the one outcome every load ends in. See
//! `specs/session.md`.

use crate::runtime::state::Runtime;
use jellysink_core::control::{Event, Origin};
use jellysink_core::jellyfin::url::redact_api_key;

pub(super) struct Load {
    item_id: Option<String>,
    origin: Origin,
}

impl Runtime {
    /// A load already up is cancelled by this one's origin, so it still gets
    /// its outcome.
    pub(super) fn begin_load(&mut self, item_id: Option<String>, origin: Origin) {
        self.cancel_load(origin);
        self.loading = Some(Load {
            item_id: item_id.clone(),
            origin,
        });
        self.settle(Event::PlayLoading { item_id, origin });
    }

    /// Publishes the status whether or not a load was up: it is how
    /// `file-loaded` ends.
    pub(super) fn finish_load(&mut self) {
        match self.loading.take() {
            Some(Load { item_id, origin }) => self.settle(Event::PlayStarted { item_id, origin }),
            None => self.publish_status(),
        }
    }

    pub(super) fn fail_load(&mut self, error: &color_eyre::Report) {
        if let Some(Load { item_id, origin }) = self.loading.take() {
            let error = redact_api_key(&format!("{error:#}"));
            self.settle(Event::PlayFailed {
                item_id,
                origin,
                error,
            });
        }
    }

    pub(super) fn cancel_load(&mut self, by: Origin) {
        if let Some(Load { item_id, origin }) = self.loading.take() {
            self.settle(Event::PlayCancelled {
                item_id,
                origin,
                by,
            });
        }
    }

    /// The status goes first, so no client sees an event followed by an older
    /// status.
    fn settle(&self, event: Event) {
        self.publish_status();
        // Fails only while no client is listening.
        let _ = self.event_tx.send(event);
    }
}

#[cfg(test)]
#[path = "loading_test.rs"]
mod tests;
