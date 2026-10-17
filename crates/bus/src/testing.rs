//! Test helpers for the bus: waiting on what it publishes, and a dead-end
//! interrupt receiver for code that drains interrupts. Behind `test-support`,
//! and compiled for this crate's own tests.

use std::fmt::Display;

use tokio::sync::mpsc;

use crate::interrupt::Interrupt;
use crate::{BusHandle, Subscriber, WorkbenchEvent, topics};
use residuum_core::testing::wait::guarded;

/// Create a dead-end receiver that will never receive any messages.
///
/// For tests that run a turn or a session without an interrupt source of its
/// own. Unbounded, matching the main agent's own channel shape: nothing is
/// ever sent through it, so a session test using it in place of its own
/// bounded channel still works, since `InterruptSource` is implemented for
/// both channel shapes.
#[must_use]
pub fn dead_interrupt_rx() -> mpsc::UnboundedReceiver<Interrupt> {
    let (_tx, rx) = mpsc::unbounded_channel::<Interrupt>();
    rx
}

/// The next event a bus subscriber receives, waiting for one under the hang
/// guard. A bus subscriber has no non-blocking receive, so it is waited on
/// here rather than through core's `Recv`.
///
/// # Panics
/// Panics, naming `what`, when the broker shuts down or the subscriber falls
/// behind before the event arrives, or when no event arrives within the hang
/// guard.
#[expect(
    clippy::panic,
    reason = "test-support helper: a bus that never delivers fails the test that waits on it"
)]
pub async fn next_event<E>(what: impl Display, sub: &mut Subscriber<E>) -> E
where
    E: Clone + Send + Sync + 'static,
{
    match guarded(&what, sub.recv()).await {
        Ok(Some(event)) => event,
        Ok(None) => panic!("the broker shut down while waiting for {what}"),
        Err(e) => panic!("a bus subscriber failed to receive while waiting for {what}: {e}"),
    }
}

/// Returns once every event `bus` was sent before this call has been handed
/// to every subscriber. The broker takes commands in order and fans each one
/// out before the next, so a barrier event that comes back proves the earlier
/// publishes were fanned out too. Then `drain` on a subscriber shows exactly
/// what it was sent.
///
/// # Panics
/// Panics when the broker is not running, or when the barrier does not come
/// back within the hang guard.
#[expect(
    clippy::expect_used,
    reason = "test-support helper: a broker that is not running fails the test that waits on it"
)]
pub async fn bus_barrier(bus: &BusHandle) {
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
    use super::*;

    #[tokio::test]
    async fn next_event_reads_the_next_bus_event() {
        let bus = crate::spawn_broker();
        let mut sub: Subscriber<WorkbenchEvent> = bus.subscribe(topics::Workbench).await.unwrap();
        bus.publisher()
            .publish(
                topics::Workbench,
                WorkbenchEvent::Updated {
                    name: "first".to_string(),
                },
            )
            .await
            .unwrap();
        assert_eq!(
            next_event("the first workbench event", &mut sub).await,
            WorkbenchEvent::Updated {
                name: "first".to_string()
            }
        );
    }

    #[tokio::test]
    async fn bus_barrier_follows_every_earlier_publish() {
        let bus = crate::spawn_broker();
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
}
