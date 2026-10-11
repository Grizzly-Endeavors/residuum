//! Awaits a future under the suite's hang guard.
//!
//! The integration-test counterpart of `crate::testing::wait::guarded`: the
//! same 60 second guard, which exists only to fail a test that would otherwise
//! hang.

use std::fmt::Display;
use std::future::IntoFuture;
use std::time::Duration;

const HANG_GUARD: Duration = Duration::from_secs(60);

/// Await `fut`, panicking if it is still pending after the hang guard.
///
/// # Panics
/// Panics if `fut` does not complete within the hang guard, naming `what`.
pub async fn guarded<F: IntoFuture>(what: impl Display, fut: F) -> F::Output {
    let outcome = tokio::time::timeout(HANG_GUARD, fut).await;
    assert!(
        outcome.is_ok(),
        "gave up after {HANG_GUARD:?} waiting for {what}"
    );
    outcome.unwrap_or_else(|_| unreachable!("the assertion above holds the output"))
}
