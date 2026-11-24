use super::ws::{Outbound, send_frame};
use crate::types::HubSocketFrame;
use residuum_bus::{BusError, BusHandle, Subscriber, WorkbenchEvent, topics};

pub(super) struct ArtifactEvents {
    feed: Option<Subscriber<WorkbenchEvent>>,
}

impl ArtifactEvents {
    pub(super) async fn subscribe(team_bus: &BusHandle) -> Self {
        let feed = match team_bus.subscribe(topics::Workbench).await {
            Ok(feed) => Some(feed),
            Err(e) => {
                tracing::warn!(error = %e, "hub websocket couldn't subscribe to artifact events; artifact lists won't update live");
                None
            }
        };
        Self { feed }
    }

    pub(super) async fn next(&mut self) -> Result<Option<WorkbenchEvent>, BusError> {
        match &mut self.feed {
            Some(feed) => feed.recv().await,
            None => std::future::pending().await,
        }
    }

    pub(super) async fn forward(
        &mut self,
        outbound: &mut Outbound,
        event: Result<Option<WorkbenchEvent>, BusError>,
    ) -> bool {
        let frame = match event {
            Ok(Some(WorkbenchEvent::Updated { name })) => HubSocketFrame::ArtifactUpdated { name },
            Ok(Some(WorkbenchEvent::Removed { name })) => HubSocketFrame::ArtifactRemoved { name },
            Ok(None) | Err(_) => {
                tracing::warn!(
                    "the artifact event feed ended; the hub websocket stops forwarding artifact events"
                );
                self.feed = None;
                return true;
            }
        };
        send_frame(outbound, &frame).await
    }
}
