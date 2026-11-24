use std::fmt::Display;

use tokio::sync::mpsc;

use crate::interrupt::Interrupt;
use crate::{BusHandle, Subscriber, WorkbenchEvent, topics};
use residuum_core::testing::wait::guarded;

#[must_use]
pub fn dead_interrupt_rx() -> mpsc::UnboundedReceiver<Interrupt> {
    let (_tx, rx) = mpsc::unbounded_channel::<Interrupt>();
    rx
}

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
