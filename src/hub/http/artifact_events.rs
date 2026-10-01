//! Artifact events on the hub WebSocket.
//!
//! The hub's own workbench watcher reads the team change feed and publishes
//! a [`WorkbenchEvent`] for every artifact that was added, changed or removed
//! (see [`crate::hub::services::TeamChangeFeed`]). Every connection forwards
//! them as `artifact_updated` and `artifact_removed`, so an artifact list or
//! a page showing one stays current with no agent running.

use super::ws::{Outbound, send_frame};
use crate::bus::{BusError, BusHandle, Subscriber, WorkbenchEvent, topics};
use crate::hub::types::HubSocketFrame;

/// A connection's end of the hub's artifact events.
pub(super) struct ArtifactEvents {
    /// `None` when the feed couldn't be subscribed to or has ended.
    feed: Option<Subscriber<WorkbenchEvent>>,
}

impl ArtifactEvents {
    /// Subscribe to the artifact events on the team bus. A connection that
    /// can't is told nothing: it keeps working and logs why.
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

    /// The next artifact event, or never when there is no feed.
    pub(super) async fn next(&mut self) -> Result<Option<WorkbenchEvent>, BusError> {
        match &mut self.feed {
            Some(feed) => feed.recv().await,
            None => std::future::pending().await,
        }
    }

    /// Send what [`Self::next`] returned. `false` when the connection should
    /// end.
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
