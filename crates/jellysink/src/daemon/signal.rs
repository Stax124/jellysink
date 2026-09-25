use tokio::sync::watch;

/// A latching one-way signal: a receiver re-creates its future across awaits as
/// long as mpv's 10 s IPC timeout, and would drop a Quit landing in that window.
#[derive(Clone, Debug)]
pub(crate) struct Signal(watch::Sender<bool>);

impl Signal {
    pub(crate) fn new() -> Self {
        Self(watch::Sender::new(false))
    }

    /// Latches the signal. Receivers that are not currently polling still see it.
    pub(crate) fn fire(&self) {
        self.0.send_replace(true);
    }

    /// Clears the latch, returning whether it was set, so an edge-triggered
    /// signal re-arms for the next fire.
    pub(crate) fn take(&self) -> bool {
        self.0.send_replace(false)
    }

    /// Resolves once the signal has been fired, whenever that happened —
    /// including before this call.
    pub(crate) async fn fired(&self) {
        // Cannot fail: `self` holds a sender, so the channel never closes.
        let _ = self.0.subscribe().wait_for(|fired| *fired).await;
    }
}

#[cfg(test)]
#[path = "signal_test.rs"]
mod tests;
