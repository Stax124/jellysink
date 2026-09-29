use super::*;
use std::time::Duration;
use tokio::time::timeout;

#[tokio::test]
async fn fired_resolves_once_the_signal_arrives_and_not_before() {
    let sig = Signal::new();
    let fired = sig.fired();
    tokio::pin!(fired);
    assert!(
        timeout(Duration::ZERO, &mut fired).await.is_err(),
        "fired() resolved without a fire()"
    );
    sig.fire();
    timeout(Duration::ZERO, fired)
        .await
        .expect("a waiting fired() should resolve once the signal arrives");
}

/// The regression this type exists for: with `Notify::notify_waiters` a
/// signal sent while nothing was polling was lost forever.
#[tokio::test]
async fn a_signal_fired_before_anyone_waits_is_not_lost() {
    let sig = Signal::new();
    sig.fire();
    timeout(Duration::ZERO, sig.fired())
        .await
        .expect("a latched signal must be observed by a later waiter");
}

/// `fired()` is polled inside `tokio::select!` arms that lose the race and
/// get dropped. That must not consume the latch.
#[tokio::test]
async fn dropping_a_fired_future_does_not_consume_the_latch() {
    let sig = Signal::new();
    assert!(timeout(Duration::ZERO, sig.fired()).await.is_err());
    sig.fire();
    timeout(Duration::ZERO, sig.fired()).await.unwrap();
    timeout(Duration::ZERO, sig.fired())
        .await
        .expect("latch must survive dropped waiters");
}

#[tokio::test]
async fn take_clears_the_latch_so_the_next_edge_is_a_fresh_wait() {
    let sig = Signal::new();
    sig.fire();
    assert!(sig.take(), "take() should report the latch was set");
    assert!(
        timeout(Duration::ZERO, sig.fired()).await.is_err(),
        "take() should have cleared the latch"
    );
    sig.fire();
    timeout(Duration::ZERO, sig.fired())
        .await
        .expect("a later fire() must be observed");
}
