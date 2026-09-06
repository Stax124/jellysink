use std::sync::Arc;
use tokio::sync::watch;

/// A latching one-way signal: shutdown, restart, or "install the update".
///
/// This replaces `Arc<Notify>` + `notify_waiters()`. `notify_waiters` stores no
/// permit — it only wakes futures that are *already registered*. Every receiver
/// in this crate re-creates its future on each loop iteration, and those loop
/// bodies routinely await mpv IPC (10 s timeout), so a tray Quit or a
/// `jellysink stop` landing in that window was silently dropped.
///
/// `watch` latches instead: [`Signal::fire`] is observed by [`Signal::fired`]
/// no matter when it was called. [`Signal::fired`] is cancel-safe — dropping
/// the future never consumes the latched value.
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
            // The sender lives as long as this `Signal`, so `Err` only happens
            // once every clone is gone. Treat that as "nothing left to wait for"
            // rather than hanging.
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
