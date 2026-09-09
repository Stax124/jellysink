use std::sync::Arc;
use tokio::sync::watch;

/// A latching one-way signal: shutdown, restart, or "install the update".
///
/// Latching matters because receivers re-create their future each iteration
/// around awaits as long as mpv's 10 s IPC timeout, so anything that only wakes
/// already-registered futures drops a Quit landing in that window.
/// [`Signal::fired`] is cancel-safe.
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

    /// Clears the latch, returning whether it was set.
    ///
    /// For edge-triggered signals (the tray's "Install update"), which must run
    /// again on the next click rather than spin on a permanently-set latch.
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
