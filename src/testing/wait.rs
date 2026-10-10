//! Waiting on events, under one hang guard.
//!
//! Every wait here ends when the thing it waits for happens. [`HANG_GUARD`]
//! is there only so a test that would otherwise hang fails with a message
//! naming what it was waiting for; it is not a measure of how long anything
//! should take, so nothing in a test should ever need a shorter one.

use std::fmt::{Debug, Display};
use std::future::{Future, IntoFuture};
use std::time::Duration;

use tokio::sync::{broadcast, mpsc, watch};

use crate::bus::WorkbenchEvent;
use crate::bus::{BusHandle, Subscriber, topics};

/// The suite's one deadline: how long a wait runs before the test is taken to
/// have hung.
pub(crate) const HANG_GUARD: Duration = Duration::from_secs(60);

/// How often [`until`] checks its condition.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// How many of the last items seen a failed [`next_where`] reports.
const SEEN_IN_FAILURE: usize = 5;

/// Await `fut`, failing the test if it hangs.
pub(crate) async fn guarded<F: IntoFuture>(what: impl Display, fut: F) -> F::Output {
    match tokio::time::timeout(HANG_GUARD, fut).await {
        Ok(output) => output,
        Err(tokio::time::error::Elapsed { .. }) => {
            panic!("gave up after {HANG_GUARD:?} waiting for {what}")
        }
    }
}

/// Poll `check` until it returns `Some`.
///
/// For state that has no channel or watch to wait on. A condition that has
/// one should use [`watch_until`] or [`next_where`] instead.
pub(crate) async fn until<T, F, Fut>(what: impl Display, mut check: F) -> T
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Option<T>>,
{
    guarded(&what, async {
        loop {
            if let Some(found) = check().await {
                return found;
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    })
    .await
}

/// [`until`], but a hang also reports what `report` says: the state the wait
/// last saw, or logs from the processes it was waiting on, so the failure can
/// be diagnosed without reproducing it.
pub(crate) async fn until_reporting<T, F, Fut>(
    what: impl Display,
    mut check: F,
    report: impl FnOnce() -> String,
) -> T
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Option<T>>,
{
    let poll = async {
        loop {
            if let Some(found) = check().await {
                return found;
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    };
    match tokio::time::timeout(HANG_GUARD, poll).await {
        Ok(found) => found,
        Err(tokio::time::error::Elapsed { .. }) => {
            panic!(
                "gave up after {HANG_GUARD:?} waiting for {what}\n{}",
                report()
            )
        }
    }
}

/// Poll `check` until it holds.
pub(crate) async fn until_true(what: impl Display, mut check: impl FnMut() -> bool) {
    until(&what, || std::future::ready(check().then_some(()))).await;
}

/// Wait until the watched value satisfies `pred`, and return it.
pub(crate) async fn watch_until<T: Clone>(
    what: impl Display,
    rx: &mut watch::Receiver<T>,
    pred: impl FnMut(&T) -> bool,
) -> T {
    match guarded(&what, rx.wait_for(pred)).await {
        Ok(value) => (*value).clone(),
        Err(watch::error::RecvError { .. }) => {
            panic!("the sender went away while waiting for {what}")
        }
    }
}

/// A receiving end a test can wait on.
pub(crate) trait Recv {
    type Item: Debug;

    /// The next item, waiting for one; `None` once the sender is gone.
    async fn recv_next(&mut self) -> Option<Self::Item>;

    /// The next item if one is already queued.
    fn try_next(&mut self) -> Option<Self::Item>;
}

impl<T: Debug> Recv for mpsc::Receiver<T> {
    type Item = T;

    async fn recv_next(&mut self) -> Option<T> {
        self.recv().await
    }

    fn try_next(&mut self) -> Option<T> {
        self.try_recv().ok()
    }
}

impl<T: Debug> Recv for mpsc::UnboundedReceiver<T> {
    type Item = T;

    async fn recv_next(&mut self) -> Option<T> {
        self.recv().await
    }

    fn try_next(&mut self) -> Option<T> {
        self.try_recv().ok()
    }
}

/// A lagged receiver missed messages, so a test reading it can't conclude
/// anything from what it saw: it fails instead.
impl<T: Clone + Debug> Recv for broadcast::Receiver<T> {
    type Item = T;

    async fn recv_next(&mut self) -> Option<T> {
        match self.recv().await {
            Ok(item) => Some(item),
            Err(broadcast::error::RecvError::Closed) => None,
            Err(broadcast::error::RecvError::Lagged(missed)) => {
                panic!("the receiver fell behind and missed {missed} messages")
            }
        }
    }

    fn try_next(&mut self) -> Option<T> {
        match self.try_recv() {
            Ok(item) => Some(item),
            Err(broadcast::error::TryRecvError::Empty | broadcast::error::TryRecvError::Closed) => {
                None
            }
            Err(broadcast::error::TryRecvError::Lagged(missed)) => {
                panic!("the receiver fell behind and missed {missed} messages")
            }
        }
    }
}

/// The next item `rx` receives.
pub(crate) async fn next<R: Recv>(what: impl Display, rx: &mut R) -> R::Item {
    match guarded(&what, rx.recv_next()).await {
        Some(item) => item,
        None => panic!("the sender went away while waiting for {what}"),
    }
}

/// Receive until an item satisfies `pred`. Returns everything received, the
/// match last, so a test can also check what came before it.
pub(crate) async fn next_where<R: Recv>(
    what: impl Display,
    rx: &mut R,
    mut pred: impl FnMut(&R::Item) -> bool,
) -> Vec<R::Item> {
    let mut seen = Vec::new();
    let found = tokio::time::timeout(HANG_GUARD, async {
        while let Some(item) = rx.recv_next().await {
            let matched = pred(&item);
            seen.push(item);
            if matched {
                return true;
            }
        }
        false
    })
    .await;
    let last: Vec<&R::Item> = seen
        .iter()
        .skip(seen.len().saturating_sub(SEEN_IN_FAILURE))
        .collect();
    match found {
        Ok(true) => seen,
        Ok(false) => {
            panic!("the sender went away while waiting for {what}; last received: {last:?}")
        }
        Err(tokio::time::error::Elapsed { .. }) => {
            panic!("gave up after {HANG_GUARD:?} waiting for {what}; last received: {last:?}")
        }
    }
}

/// Everything already queued on `rx`, without waiting. Meaningful only after
/// a sync point that proves everything the test cares about has been sent.
pub(crate) fn drain<R: Recv>(rx: &mut R) -> Vec<R::Item> {
    std::iter::from_fn(|| rx.try_next()).collect()
}

/// Returns once every event `bus` was sent before this call has been handed
/// to every subscriber. The broker takes commands in order and fans each one
/// out before the next, so a barrier event that comes back proves the earlier
/// publishes were fanned out too. Then `drain` on a subscriber shows exactly
/// what it was sent.
pub(crate) async fn bus_barrier(bus: &BusHandle) {
    let mut barrier: Subscriber<WorkbenchEvent> = bus
        .subscribe(topics::Workbench)
        .await
        .expect("the broker is running");
    bus.publisher()
        .publish(
            topics::Workbench,
            WorkbenchEvent::Updated {
                name: "barrier".to_string(),
            },
        )
        .await
        .expect("the broker is running");
    guarded("the bus barrier", barrier.recv())
        .await
        .expect("the broker is running");
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[tokio::test]
    async fn guarded_returns_what_the_future_does() {
        assert_eq!(guarded("a ready value", async { 7 }).await, 7);
    }

    #[tokio::test(start_paused = true)]
    #[should_panic(expected = "gave up after 60s waiting for something that never comes")]
    async fn guarded_names_what_it_waited_for() {
        guarded("something that never comes", std::future::pending::<()>()).await;
    }

    #[tokio::test]
    async fn until_returns_the_first_value_found() {
        let calls = AtomicUsize::new(0);
        let found = until("the third check", || {
            let n = calls.fetch_add(1, Ordering::Relaxed);
            std::future::ready((n == 2).then_some(n))
        })
        .await;
        assert_eq!(found, 2);
    }

    #[tokio::test]
    async fn until_true_returns_once_the_condition_holds() {
        let flag = Arc::new(AtomicUsize::new(0));
        let setter = Arc::clone(&flag);
        crate::util::spawn_in_span(async move { setter.store(1, Ordering::Relaxed) });
        until_true("the flag to be set", || flag.load(Ordering::Relaxed) == 1).await;
    }

    #[tokio::test]
    async fn watch_until_returns_the_matching_value() {
        let (tx, mut rx) = watch::channel(0);
        crate::util::spawn_in_span(async move {
            for n in 1..=3 {
                tx.send_replace(n);
                tokio::task::yield_now().await;
            }
        });
        assert_eq!(watch_until("the value 3", &mut rx, |n| *n == 3).await, 3);
    }

    #[tokio::test]
    #[should_panic(expected = "the sender went away while waiting for a value that never comes")]
    async fn watch_until_fails_when_the_sender_is_gone() {
        let (tx, mut rx) = watch::channel(0);
        drop(tx);
        watch_until("a value that never comes", &mut rx, |n| *n == 1).await;
    }

    #[tokio::test]
    async fn next_and_drain_read_an_mpsc_channel() {
        let (tx, mut rx) = mpsc::channel(4);
        for n in 1..=3 {
            tx.send(n).await.unwrap();
        }
        assert_eq!(next("the first item", &mut rx).await, 1);
        assert_eq!(drain(&mut rx), vec![2, 3]);
    }

    #[tokio::test]
    async fn next_where_returns_everything_up_to_the_match() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        for n in 1..=4 {
            tx.send(n).unwrap();
        }
        assert_eq!(
            next_where("an even item past 2", &mut rx, |n| *n == 4).await,
            vec![1, 2, 3, 4]
        );
        assert!(drain(&mut rx).is_empty());
    }

    #[tokio::test]
    #[should_panic(expected = "last received: [1, 2]")]
    async fn next_where_reports_what_it_saw_when_the_sender_goes() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        tx.send(1).unwrap();
        tx.send(2).unwrap();
        drop(tx);
        next_where("a 3", &mut rx, |n| *n == 3).await;
    }

    #[tokio::test]
    async fn bus_barrier_follows_every_earlier_publish() {
        let bus = crate::bus::spawn_broker();
        let mut sub: Subscriber<WorkbenchEvent> = bus.subscribe(topics::Workbench).await.unwrap();
        bus.publisher()
            .publish(
                topics::Workbench,
                WorkbenchEvent::Updated {
                    name: "earlier".to_string(),
                },
            )
            .await
            .unwrap();
        bus_barrier(&bus).await;
        assert_eq!(
            sub.drain(),
            vec![
                WorkbenchEvent::Updated {
                    name: "earlier".to_string()
                },
                WorkbenchEvent::Updated {
                    name: "barrier".to_string()
                },
            ]
        );
    }

    #[tokio::test]
    async fn a_broadcast_receiver_reads_in_order() {
        let (tx, mut rx) = broadcast::channel(4);
        tx.send("a").unwrap();
        tx.send("b").unwrap();
        assert_eq!(next("the first message", &mut rx).await, "a");
        assert_eq!(drain(&mut rx), vec!["b"]);
    }

    #[tokio::test]
    #[should_panic(expected = "missed 1 messages")]
    async fn a_lagged_broadcast_receiver_fails_the_test() {
        let (tx, mut rx) = broadcast::channel(1);
        tx.send(1).unwrap();
        tx.send(2).unwrap();
        drain(&mut rx);
    }
}
