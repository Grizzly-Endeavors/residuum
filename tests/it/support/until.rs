//! Polls a check until it finds something, under the suite's hang guard.
//!
//! The integration-test counterpart of `crate::testing::wait::until`: the same
//! 60 second guard, which exists only to fail a test that would otherwise hang.

use std::fmt::Display;
use std::future::Future;
use std::time::Duration;

const HANG_GUARD: Duration = Duration::from_secs(60);
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Poll `check` until it returns `Some`, and return the value it found.
///
/// # Panics
/// Panics if nothing is found within the hang guard, naming `what`.
pub async fn until<T, F, Fut>(what: impl Display, mut check: F) -> T
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
    let outcome = tokio::time::timeout(HANG_GUARD, poll).await;
    assert!(
        outcome.is_ok(),
        "gave up after {HANG_GUARD:?} waiting for {what}"
    );
    outcome.unwrap_or_else(|_| unreachable!("the assertion above holds the found value"))
}
