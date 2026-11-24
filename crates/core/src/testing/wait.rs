use std::fmt::{Debug, Display};
use std::future::{Future, IntoFuture};
use std::time::Duration;

use tokio::sync::{broadcast, mpsc, watch};

pub const HANG_GUARD: Duration = Duration::from_secs(60);

const POLL_INTERVAL: Duration = Duration::from_millis(10);

const SEEN_IN_FAILURE: usize = 5;

#[expect(
    clippy::panic,
    reason = "test-support helper: a hung wait fails the test that called it"
)]
pub async fn guarded<F: IntoFuture>(what: impl Display, fut: F) -> F::Output {
    match tokio::time::timeout(HANG_GUARD, fut).await {
        Ok(output) => output,
        Err(tokio::time::error::Elapsed { .. }) => {
            panic!("gave up after {HANG_GUARD:?} waiting for {what}")
        }
    }
}

pub async fn until<T, F, Fut>(what: impl Display, mut check: F) -> T
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

#[expect(
    clippy::panic,
    reason = "test-support helper: a hung wait fails the test that called it"
)]
pub async fn until_reporting<T, F, Fut>(
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

pub async fn until_true(what: impl Display, mut check: impl FnMut() -> bool) {
    until(&what, || std::future::ready(check().then_some(()))).await;
}

#[expect(
    clippy::panic,
    reason = "test-support helper: a wait that can't complete fails the test that called it"
)]
pub async fn watch_until<T: Clone>(
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

#[expect(
    async_fn_in_trait,
    reason = "test-support helper: tests await it on their own task, so no caller needs a `Send` bound on the future"
)]
pub trait Recv {
    type Item;

    async fn recv_next(&mut self) -> Option<Self::Item>;

    fn try_next(&mut self) -> Option<Self::Item>;
}

impl<T> Recv for mpsc::Receiver<T> {
    type Item = T;

    async fn recv_next(&mut self) -> Option<T> {
        self.recv().await
    }

    fn try_next(&mut self) -> Option<T> {
        self.try_recv().ok()
    }
}

impl<T> Recv for mpsc::UnboundedReceiver<T> {
    type Item = T;

    async fn recv_next(&mut self) -> Option<T> {
        self.recv().await
    }

    fn try_next(&mut self) -> Option<T> {
        self.try_recv().ok()
    }
}

#[expect(
    clippy::panic,
    reason = "test-support helper: a receiver that missed messages fails the test reading it"
)]
impl<T: Clone> Recv for broadcast::Receiver<T> {
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

#[expect(
    clippy::panic,
    reason = "test-support helper: a wait that can't complete fails the test that called it"
)]
pub async fn next<R: Recv>(what: impl Display, rx: &mut R) -> R::Item {
    match guarded(&what, rx.recv_next()).await {
        Some(item) => item,
        None => panic!("the sender went away while waiting for {what}"),
    }
}

#[expect(
    clippy::panic,
    reason = "test-support helper: a wait that can't complete fails the test that called it"
)]
pub async fn next_matching<R: Recv>(
    what: impl Display,
    rx: &mut R,
    mut pred: impl FnMut(&R::Item) -> bool,
) -> R::Item {
    guarded(&what, async {
        loop {
            match rx.recv_next().await {
                Some(item) if pred(&item) => return item,
                Some(_) => {}
                None => panic!("the sender went away while waiting for {what}"),
            }
        }
    })
    .await
}

#[expect(
    clippy::panic,
    reason = "test-support helper: a wait that can't complete fails the test that called it"
)]
pub async fn next_where<R: Recv>(
    what: impl Display,
    rx: &mut R,
    mut pred: impl FnMut(&R::Item) -> bool,
) -> Vec<R::Item>
where
    R::Item: Debug,
{
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

pub fn drain<R: Recv>(rx: &mut R) -> Vec<R::Item> {
    std::iter::from_fn(|| rx.try_next()).collect()
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
    async fn next_matching_skips_to_the_first_match_and_leaves_the_rest_queued() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        for n in 1..=4 {
            tx.send(n).unwrap();
        }
        assert_eq!(
            next_matching("an item past 2", &mut rx, |n| *n > 2).await,
            3
        );
        assert_eq!(drain(&mut rx), vec![4]);
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
        let actual = &drain(&mut rx);
        assert!(actual.is_empty(), "expected {actual:?} to be empty");
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
