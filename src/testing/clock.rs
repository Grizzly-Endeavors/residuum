//! Moving a paused clock.
//!
//! For tests of code with no real I/O (no sockets, child processes or OS file
//! watchers), run on a paused clock (`#[tokio::test(start_paused = true)]`)
//! and move time with these. Each checks first that the clock is paused, so
//! it can never quietly measure real time. Under a paused clock, a runtime
//! with nothing to do jumps straight to its next timer, so waiting on a socket
//! there fires every pending timeout at once; keep real I/O out of these tests.

use std::future::IntoFuture;
use std::time::Duration;

/// Let `duration` of paused time pass, firing every timer due in it in order.
///
/// This sleeps rather than calling `tokio::time::advance`: a sleep lets each
/// woken task run before time moves on, so timers fire in order, where an
/// advance jumps past them all at once.
///
/// # Panics
/// Panics if the clock isn't paused.
pub(crate) async fn elapse(duration: Duration) {
    assert_paused().await;
    tokio::time::sleep(duration).await;
}

/// `fut`'s output if it completes within `duration` of paused time, `None` if
/// it doesn't. On a paused clock this is how a test checks that something
/// does not happen within a window.
///
/// # Panics
/// Panics if the clock isn't paused.
pub(crate) async fn within<F: IntoFuture>(duration: Duration, fut: F) -> Option<F::Output> {
    assert_paused().await;
    tokio::time::timeout(duration, fut).await.ok()
}

/// `tokio::time::advance` panics with "time is not frozen" when the clock
/// runs in real time, which is the check wanted here; advancing by zero
/// changes nothing else.
async fn assert_paused() {
    tokio::time::advance(Duration::ZERO).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn elapse_fires_timers_due_in_the_window_in_order() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        for (label, after) in [("second", 20), ("first", 10), ("late", 50)] {
            let tx = tx.clone();
            crate::util::spawn_in_span(async move {
                tokio::time::sleep(Duration::from_millis(after)).await;
                tx.send(label).unwrap();
            });
        }
        elapse(Duration::from_millis(30)).await;
        assert_eq!(
            crate::testing::wait::drain(&mut rx),
            vec!["first", "second"]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn within_tells_a_timely_future_from_a_late_one() {
        let late = tokio::time::sleep(Duration::from_secs(5));
        assert_eq!(within(Duration::from_secs(1), late).await, None);
        assert_eq!(within(Duration::from_secs(1), async { 3 }).await, Some(3));
    }

    #[tokio::test]
    #[should_panic(expected = "time is not frozen")]
    async fn elapse_refuses_a_real_clock() {
        elapse(Duration::from_millis(1)).await;
    }
}
