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
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn fired_resolves_when_the_signal_arrives_later() {
        let sig = Signal::new();
        let bg = sig.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            bg.fire();
        });
        tokio::time::timeout(Duration::from_secs(5), sig.fired())
            .await
            .expect("fired should resolve once the signal arrives");
    }

    /// The regression this type exists for: with `Notify::notify_waiters` a
    /// signal sent while nothing was polling was lost forever.
    #[tokio::test]
    async fn a_signal_fired_before_anyone_waits_is_not_lost() {
        let sig = Signal::new();
        sig.fire();
        tokio::time::timeout(Duration::from_secs(5), sig.fired())
            .await
            .expect("a latched signal must be observed by a later waiter");
    }

    /// `fired()` is polled inside `tokio::select!` arms that lose the race and
    /// get dropped. That must not consume the latch.
    #[tokio::test]
    async fn dropping_a_fired_future_does_not_consume_the_latch() {
        let sig = Signal::new();
        {
            let fut = sig.fired();
            drop(fut);
        }
        sig.fire();
        {
            let fut = sig.fired();
            drop(fut);
        }
        tokio::time::timeout(Duration::from_secs(5), sig.fired())
            .await
            .expect("latch must survive dropped waiters");
    }

    #[tokio::test]
    async fn fired_does_not_resolve_before_the_signal() {
        let sig = Signal::new();
        let r = tokio::time::timeout(Duration::from_millis(50), sig.fired()).await;
        assert!(r.is_err(), "fired() resolved without a fire()");
    }

    #[tokio::test]
    async fn take_clears_the_latch_so_the_next_edge_is_a_fresh_wait() {
        let sig = Signal::new();
        sig.fire();
        assert!(sig.take(), "take() should report the latch was set");
        let r = tokio::time::timeout(Duration::from_millis(50), sig.fired()).await;
        assert!(r.is_err(), "take() should have cleared the latch");
        // And a second edge re-arms it.
        sig.fire();
        tokio::time::timeout(Duration::from_secs(5), sig.fired())
            .await
            .expect("a later fire() must be observed");
    }

    #[tokio::test]
    async fn clones_share_one_latch() {
        let a = Signal::new();
        let b = a.clone();
        a.fire();
        tokio::time::timeout(Duration::from_secs(5), b.fired())
            .await
            .expect("clone should observe the original's fire()");
    }
}
