//! Turns what the hub learns into changes to the team overview.
//!
//! The tracker reads the hub bus (agents starting, stopping, appearing and
//! going away) and the feed of agent changes (sessions, inbox additions,
//! turns), tells [`TeamOverview`] which part of which agent's overview they
//! affect, and sends the frames that come due.

use std::sync::Arc;

use tokio::sync::broadcast::{self, error::RecvError};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use super::service::{Part, TeamOverview};
use crate::hub::agent_watch::{AgentChange, AgentChangeKind, AgentChangeReceiver, WatchedPath};
use crate::hub::types::HubEvent;

/// A running tracker. It stops when dropped.
pub(crate) struct OverviewTracker {
    task: JoinHandle<()>,
}

impl OverviewTracker {
    /// Keep `overview` current from what arrives on `hub_events` and `changes`.
    ///
    /// Both receivers must be subscribed before anything they should hear
    /// about happens: neither replays. Subscribe before the agents start.
    pub(crate) fn spawn(
        overview: Arc<TeamOverview>,
        hub_events: broadcast::Receiver<HubEvent>,
        changes: AgentChangeReceiver,
    ) -> Self {
        Self {
            task: crate::util::spawn_monitored("team-overview", run(overview, hub_events, changes)),
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

/// Waits until `at`, or forever when there is nothing to wait for.
async fn wait_until(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

async fn on_hub_event(overview: &TeamOverview, event: HubEvent) {
    match event {
        // A state change moves what an agent shows from its files to its
        // running parts or back, and its sessions end with it.
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

/// The parts of an agent's overview that `kind` may have changed.
fn parts_changed_by(kind: &AgentChangeKind) -> &'static [Part] {
    match kind {
        AgentChangeKind::Resync => &Part::ALL,
        AgentChangeKind::SessionStarted(_)
        | AgentChangeKind::SessionStateChanged { .. }
        | AgentChangeKind::SessionCompleted { .. } => &[Part::Sessions],
        // The tool's save also arrives as a file change; either one is
        // enough to count the inbox again.
        AgentChangeKind::UserInboxAdded { .. }
        | AgentChangeKind::WatchedPathChanged(WatchedPath::UserInbox) => &[Part::Inbox],
        // A pulse that ran moves its next run, and an edit to the pulses, the
        // actions or the settings adds, removes or moves runs. A pulse system
        // that is switched off in the settings has none.
        AgentChangeKind::WatchedPathChanged(
            WatchedPath::ScheduledActions
            | WatchedPath::Heartbeat
            | WatchedPath::PulseState
            | WatchedPath::Config,
        ) => &[Part::Upcoming],
        // A task's unreachable streak starting, passing the notice threshold
        // and ending all arrive this way. The service also waits for the
        // threshold itself, for the moment before the tracker announces it.
        AgentChangeKind::OutboundTaskChanged(_) => &[Part::OutboundProblems],
        // A turn changes the last message, which `turn_ended` records. No
        // other part of the overview reads the rest.
        AgentChangeKind::TurnEnded(_) => &[],
    }
}
