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
