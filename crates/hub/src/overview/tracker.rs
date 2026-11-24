use std::sync::Arc;

use tokio::sync::broadcast::{self, error::RecvError};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use super::service::{Part, TeamOverview};
use crate::agent_watch::{AgentChange, AgentChangeKind, AgentChangeReceiver, WatchedPath};
use crate::types::HubEvent;

pub struct OverviewTracker {
    task: JoinHandle<()>,
}

impl OverviewTracker {
    pub fn spawn(
        overview: Arc<TeamOverview>,
        hub_events: broadcast::Receiver<HubEvent>,
        changes: AgentChangeReceiver,
    ) -> Self {
        Self {
            task: residuum_core::util::spawn_monitored(
                "team-overview",
                run(overview, hub_events, changes),
            ),
        }
    }
}

impl Drop for OverviewTracker {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn run(
    overview: Arc<TeamOverview>,
    mut hub_events: broadcast::Receiver<HubEvent>,
    mut changes: AgentChangeReceiver,
) {
    loop {
        let next_due = overview.next_due().await;
        tokio::select! {
            event = hub_events.recv() => match event {
                Ok(event) => on_hub_event(&overview, event).await,
                Err(RecvError::Lagged(missed)) => {
                    tracing::warn!(missed, "the team overview fell behind the hub's events; reading every agent again");
                    overview.changed_everywhere().await;
                }
                Err(RecvError::Closed) => break,
            },
            change = changes.recv() => match change {
                Some(change) => on_agent_change(&overview, change).await,
                None => break,
            },
            () = wait_until(next_due) => overview.send_due().await,
            () = overview.due_changed() => {}
        }
    }
    tracing::debug!("the team overview stopped tracking");
}

async fn wait_until(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

async fn on_hub_event(overview: &TeamOverview, event: HubEvent) {
    match event {
        HubEvent::AgentState { agent } => overview.changed(&agent.name, &Part::ALL).await,
        HubEvent::AgentCreated { agent, .. } | HubEvent::AgentRestored { agent, .. } => {
            overview.announce(&agent.name).await;
        }
        HubEvent::AgentDeleted { name, .. } => overview.forget(&name).await,
        HubEvent::AgentStopping { .. }
        | HubEvent::AgentActivity { .. }
        | HubEvent::Notice { .. }
        | HubEvent::HubConfigReloaded { .. } => {}
    }
}

async fn on_agent_change(overview: &TeamOverview, change: AgentChange) {
    let AgentChange { agent, kind } = change;
    if let AgentChangeKind::TurnEnded(turn) = &kind {
        overview.turn_ended(&agent, turn).await;
    }
    overview.changed(&agent, parts_changed_by(&kind)).await;
}

fn parts_changed_by(kind: &AgentChangeKind) -> &'static [Part] {
    match kind {
        AgentChangeKind::Resync => &Part::ALL,
        AgentChangeKind::SessionStarted(_)
        | AgentChangeKind::SessionStateChanged { .. }
        | AgentChangeKind::SessionCompleted { .. } => &[Part::Sessions],
        AgentChangeKind::UserInboxAdded { .. }
        | AgentChangeKind::WatchedPathChanged(WatchedPath::UserInbox) => &[Part::Inbox],
        AgentChangeKind::WatchedPathChanged(
            WatchedPath::ScheduledActions
            | WatchedPath::Heartbeat
            | WatchedPath::PulseState
            | WatchedPath::Config,
        ) => &[Part::Upcoming],
        AgentChangeKind::OutboundTaskChanged(_) => &[Part::OutboundProblems],
        AgentChangeKind::TurnEnded(_) | AgentChangeKind::PulseFireFinished { .. } => &[],
    }
}
