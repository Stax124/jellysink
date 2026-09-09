/// A background task tied to the lifetime of whatever owns this; dropping it
/// aborts the task, so a reconnect cannot leave the old reader running and a
/// respawn cannot leave the old mpv forwarder running.
pub(super) struct AbortOnDrop(pub(super) tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}
