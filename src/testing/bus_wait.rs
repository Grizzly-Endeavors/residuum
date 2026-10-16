//! Waiting on what the bus and the hub publish, under the same hang guard as
//! core's `testing::wait`.

use std::fmt::Display;

use crate::bus::{BusHandle, Subscriber, WorkbenchEvent, topics};
use crate::hub::agent_watch::{AgentChange, AgentChangeReceiver};
use residuum_core::testing::wait::{Recv, guarded};

impl Recv for AgentChangeReceiver {
    type Item = AgentChange;

    async fn recv_next(&mut self) -> Option<Self::Item> {
        self.recv().await
    }

    fn try_next(&mut self) -> Option<Self::Item> {
        self.try_recv()
    }
}

/// The next event a bus subscriber receives, waiting for one. A bus subscriber
/// has no non-blocking receive, so it is waited on here rather than through
/// [`Recv`].
pub(crate) async fn next_event<E>(what: impl Display, sub: &mut Subscriber<E>) -> E
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
    use super::*;

    #[tokio::test]
    async fn next_event_reads_the_next_bus_event() {
        let bus = crate::bus::spawn_broker();
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
}
