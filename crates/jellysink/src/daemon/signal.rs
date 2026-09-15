use std::sync::Arc;
use tokio::sync::watch;

/// A latching one-way signal: a receiver re-creates its future across awaits as
/// long as mpv's 10 s IPC timeout, and would drop a Quit landing in that window.
#[derive(Clone, Debug)]
pub(crate) struct Signal {
    tx: Arc<watch::Sender<bool>>,
}

impl Signal {
    pub(crate) fn new() -> Self {
        Self {
            tx: Arc::new(watch::channel(false).0),
        }
    }

    /// Latches the signal. Receivers that are not currently polling still see it.
    pub(crate) fn fire(&self) {
        self.tx.send_replace(true);
    }

    /// Clears the latch, returning whether it was set, so an edge-triggered
    /// signal re-arms for the next fire.
    pub(crate) fn take(&self) -> bool {
        self.tx.send_replace(false)
    }

    /// Resolves once the signal has been fired, whenever that happened —
    /// including before this call.
    pub(crate) async fn fired(&self) {
        let mut rx = self.tx.subscribe();
        loop {
            if *rx.borrow_and_update() {
                return;
            }
            // Every `Signal` clone is gone: nothing left to wait for.
            if rx.changed().await.is_err() {
                return;
            }
        }
    }
}

impl Default for Signal {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "signal_test.rs"]
mod tests;
