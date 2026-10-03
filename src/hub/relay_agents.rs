//! Keeps the relay's copy of the hub's agent list current.
//!
//! The relay routes `/a2a/{instance}/{agent}` requests and builds its A2A
//! directory from the list the tunnel sends after every (re)connect. This
//! module computes that list from the agent directory and republishes it
//! whenever an agent is created or deleted, changes state, or changes A2A
//! visibility, or the hub's `[a2a] enabled` flips. The tunnel sends whatever
//! list is current; see `docs/systems-usage/cloud-tunnel.md`.
//!
//! An agent is advertised as A2A-enabled while the hub's A2A listener is
//! enabled and the agent is running. A stopped or failed agent can't answer,
//! so the relay hides it from its directory and answers `404` for it instead
//! of forwarding requests the listener would refuse. An agent whose stop has
//! begun (see [`HubEvent::AgentStopping`]) is hidden the same way, even
//! though it still reports `Running`, so the relay stops forwarding to it as
//! soon as the team router does rather than waiting for the stop to finish.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use super::directory::AgentDirectory;
use super::types::{A2aVisibility, AgentState, AgentSummary, HubEvent};
use crate::tunnel::protocol::AgentInfo;

/// How long a burst of changes is allowed to gather before one update goes
/// out. Creating an agent publishes a creation and then a start-up state
/// change within moments; this makes them one update.
const SETTLE: Duration = Duration::from_millis(250);

/// The relay-facing description of `agents`: one entry per agent, in the
/// order given, with `a2a_enabled` set for running agents when the hub's A2A
/// listener is enabled, excluding names in `stopping` (see
/// [`AgentDirectory::stopping`]). `name` is the folder; `display_name` is
/// the name people see.
#[must_use]
pub(crate) fn agent_infos(
    agents: &[AgentSummary],
    a2a_listener_enabled: bool,
    stopping: &HashSet<String>,
) -> Vec<AgentInfo> {
    agents
        .iter()
        .map(|agent| AgentInfo {
            name: agent.name.clone(),
            display_name: agent.label().to_string(),
            a2a_enabled: a2a_listener_enabled
                && agent.state == AgentState::Running
                && !stopping.contains(&agent.name),
            a2a_private: agent.a2a_visibility == A2aVisibility::Private,
            teams_configured: agent.teams_configured,
        })
        .collect()
}

/// `directory`'s currently-stopping agent names, as a set [`agent_infos`]
/// can check against.
fn stopping_set(directory: &dyn AgentDirectory) -> HashSet<String> {
    directory.stopping().into_iter().collect()
}

/// Whether `event` can change what [`agent_infos`] returns.
fn changes_agent_list(event: &HubEvent) -> bool {
    matches!(
        event,
        HubEvent::AgentState { .. }
            | HubEvent::AgentStopping { .. }
            | HubEvent::AgentCreated { .. }
            | HubEvent::AgentRestored { .. }
            | HubEvent::AgentDeleted { .. }
    )
}

/// The hub's current agent list for the relay, and the handles that keep it
/// current.
pub(crate) struct RelayAgents {
    list: Arc<watch::Sender<Vec<AgentInfo>>>,
    a2a_enabled: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl RelayAgents {
    /// Compute the current list from `directory` and start keeping it
    /// current. `a2a_listener_enabled` is the hub's `[a2a] enabled`.
    pub(crate) fn spawn(directory: Arc<dyn AgentDirectory>, a2a_listener_enabled: bool) -> Self {
        // Subscribed before the list is read, so a change between the two
        // is seen as an event rather than lost.
        let events = directory.subscribe();
        let (a2a_enabled, a2a_enabled_rx) = watch::channel(a2a_listener_enabled);
        let initial = agent_infos(
            &directory.list(),
            a2a_listener_enabled,
            &stopping_set(&*directory),
        );
        let list = Arc::new(watch::channel(initial).0);
        let task = crate::util::spawn_monitored(
            "relay-agents",
            keep_current(directory, events, a2a_enabled_rx, Arc::clone(&list)),
        );
        Self {
            list,
            a2a_enabled,
            task,
        }
    }

    /// A receiver on the current list, for a tunnel to send from.
    pub(crate) fn subscribe(&self) -> watch::Receiver<Vec<AgentInfo>> {
        self.list.subscribe()
    }

    /// Record a change to the hub's `[a2a] enabled`.
    pub(crate) fn set_a2a_listener_enabled(&self, enabled: bool) {
        self.a2a_enabled.send_if_modified(|current| {
            let changed = *current != enabled;
            *current = enabled;
            changed
        });
    }

    /// Stop keeping the list current.
    pub(crate) fn stop(&self) {
        self.task.abort();
    }
}

/// Recompute the list on every relevant hub event and every change to the
/// A2A switch, settling for [`SETTLE`] first so a burst becomes one update.
async fn keep_current(
    directory: Arc<dyn AgentDirectory>,
    mut events: broadcast::Receiver<HubEvent>,
    mut a2a_enabled: watch::Receiver<bool>,
    list: Arc<watch::Sender<Vec<AgentInfo>>>,
) {
    loop {
        tokio::select! {
            event = events.recv() => match event {
                Ok(event) if changes_agent_list(&event) => {}
                Ok(_) => continue,
                Err(RecvError::Lagged(missed)) => {
                    tracing::debug!(missed, "relay agent list republished after missing hub events");
                }
                Err(RecvError::Closed) => return,
            },
            changed = a2a_enabled.changed() => {
                if changed.is_err() {
                    return;
                }
            }
        }
        if !settle(&mut events).await {
            return;
        }
        let infos = agent_infos(
            &directory.list(),
            *a2a_enabled.borrow_and_update(),
            &stopping_set(&*directory),
        );
        list.send_if_modified(|current| {
            let changed = *current != infos;
            if changed {
                *current = infos;
            }
            changed
        });
    }
}

/// Wait out [`SETTLE`], draining the events that arrive meanwhile. `false`
/// when the event channel closed.
async fn settle(events: &mut broadcast::Receiver<HubEvent>) -> bool {
    let deadline = Instant::now() + SETTLE;
    loop {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Err(_) => return true,
            Ok(Err(RecvError::Closed)) => return false,
            Ok(Ok(_) | Err(RecvError::Lagged(_))) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a2a::StaticAgentDirectory;
    use axum::Router;

    /// Bound on every wait, so a missing update fails the test instead of
    /// hanging the suite.
    const TEST_TIMEOUT: Duration = Duration::from_secs(5);

    fn summary(name: &str, state: AgentState, visibility: A2aVisibility) -> AgentSummary {
        AgentSummary {
            name: name.to_string(),
            display_name: name.to_string(),
            state,
            last_error: None,
            autostart: true,
            role: None,
            a2a_visibility: visibility,
            teams_configured: false,
        }
    }

    fn directory(agents: &[(&str, A2aVisibility)]) -> Arc<StaticAgentDirectory> {
        let mut directory = StaticAgentDirectory::new();
        for (name, visibility) in agents {
            directory = directory.with_agent(*name, *visibility, Router::new());
        }
        Arc::new(directory)
    }

    async fn next_list(rx: &mut watch::Receiver<Vec<AgentInfo>>) -> Vec<AgentInfo> {
        tokio::time::timeout(TEST_TIMEOUT, rx.changed())
            .await
            .expect("no agent list update arrived")
            .expect("the list sender was dropped");
        rx.borrow_and_update().clone()
    }

    async fn no_update_within(rx: &mut watch::Receiver<Vec<AgentInfo>>, window: Duration) {
        assert!(
            tokio::time::timeout(window, rx.changed()).await.is_err(),
            "an unrelated event must not produce an update"
        );
    }

    #[test]
    fn running_agents_are_a2a_enabled_only_while_the_listener_is_on() {
        let agents = [
            summary("scout", AgentState::Running, A2aVisibility::Public),
            summary("archivist", AgentState::Stopped, A2aVisibility::Private),
            summary("nova", AgentState::Failed, A2aVisibility::Public),
            summary("atlas", AgentState::Starting, A2aVisibility::Public),
        ];
        let none_stopping = HashSet::new();
        let on = agent_infos(&agents, true, &none_stopping);
        assert_eq!(
            on.iter().map(|a| a.a2a_enabled).collect::<Vec<_>>(),
            [true, false, false, false]
        );
        assert_eq!(
            on.iter().map(|a| a.a2a_private).collect::<Vec<_>>(),
            [false, true, false, false]
        );
        assert!(
            agent_infos(&agents, false, &none_stopping)
                .iter()
                .all(|a| !a.a2a_enabled)
        );
    }

    #[test]
    fn a_stopped_agent_with_teams_configured_stays_configured() {
        let mut agent = summary("scout", AgentState::Stopped, A2aVisibility::Public);
        agent.teams_configured = true;
        let infos = agent_infos(&[agent], true, &HashSet::new());
        let scout = infos.first().expect("scout");
        assert!(scout.teams_configured);
        assert!(!scout.a2a_enabled);
    }

    #[test]
    fn a_running_agent_that_has_begun_stopping_is_not_a2a_enabled() {
        let agents = [summary("scout", AgentState::Running, A2aVisibility::Public)];
        let stopping = HashSet::from(["scout".to_string()]);
        let infos = agent_infos(&agents, true, &stopping);
        assert_eq!(infos.first().map(|a| a.a2a_enabled), Some(false));
    }

    #[test]
    fn the_display_name_is_the_agent_name() {
        let infos = agent_infos(
            &[summary("scout", AgentState::Running, A2aVisibility::Public)],
            true,
            &HashSet::new(),
        );
        assert_eq!(
            infos.first().map(|a| a.display_name.as_str()),
            Some("scout")
        );
    }

    #[tokio::test]
    async fn the_initial_list_describes_every_agent() {
        let directory = directory(&[
            ("scout", A2aVisibility::Public),
            ("archivist", A2aVisibility::Private),
        ]);
        let relay_agents = RelayAgents::spawn(directory, true);
        let rx = relay_agents.subscribe();
        let list = rx.borrow().clone();
        let names: Vec<&str> = list.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["archivist", "scout"]);
        assert!(list.iter().all(|a| a.a2a_enabled));
        assert_eq!(
            list.iter()
                .find(|a| a.name == "archivist")
                .map(|a| a.a2a_private),
            Some(true)
        );
        relay_agents.stop();
    }

    #[tokio::test]
    async fn creating_an_agent_republishes_the_list() {
        let directory = directory(&[("scout", A2aVisibility::Public)]);
        let relay_agents = RelayAgents::spawn(Arc::clone(&directory) as _, true);
        let mut rx = relay_agents.subscribe();

        directory.add_agent("nova", A2aVisibility::Private, Router::new());

        let list = next_list(&mut rx).await;
        let names: Vec<&str> = list.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["nova", "scout"]);
        relay_agents.stop();
    }

    #[tokio::test]
    async fn deleting_an_agent_republishes_the_list() {
        let directory = directory(&[
            ("scout", A2aVisibility::Public),
            ("nova", A2aVisibility::Public),
        ]);
        let relay_agents = RelayAgents::spawn(Arc::clone(&directory) as _, true);
        let mut rx = relay_agents.subscribe();

        directory.remove_agent("nova");

        let list = next_list(&mut rx).await;
        let names: Vec<&str> = list.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["scout"]);
        relay_agents.stop();
    }

    #[tokio::test]
    async fn stopping_and_starting_an_agent_toggles_its_a2a_enablement() {
        let directory = directory(&[("scout", A2aVisibility::Public)]);
        let relay_agents = RelayAgents::spawn(Arc::clone(&directory) as _, true);
        let mut rx = relay_agents.subscribe();

        directory.set_state("scout", AgentState::Stopped);
        let stopped = next_list(&mut rx).await;
        assert_eq!(stopped.first().map(|a| a.a2a_enabled), Some(false));

        directory.set_state("scout", AgentState::Running);
        let running = next_list(&mut rx).await;
        assert_eq!(running.first().map(|a| a.a2a_enabled), Some(true));
        relay_agents.stop();
    }

    #[tokio::test]
    async fn an_agent_that_begins_stopping_is_disabled_before_it_finishes() {
        let directory = directory(&[("scout", A2aVisibility::Public)]);
        let relay_agents = RelayAgents::spawn(Arc::clone(&directory) as _, true);
        let mut rx = relay_agents.subscribe();

        // The stop begins, but the agent still reports `Running` until its
        // event loop actually exits.
        directory.begin_stopping("scout");
        let stopping = next_list(&mut rx).await;
        assert_eq!(stopping.first().map(|a| a.a2a_enabled), Some(false));

        // Restarted without ever finishing the stop: re-enabled again.
        directory.set_state("scout", AgentState::Running);
        let running = next_list(&mut rx).await;
        assert_eq!(running.first().map(|a| a.a2a_enabled), Some(true));
        relay_agents.stop();
    }

    #[tokio::test]
    async fn a_visibility_change_republishes_the_list() {
        let directory = directory(&[("scout", A2aVisibility::Public)]);
        let relay_agents = RelayAgents::spawn(Arc::clone(&directory) as _, true);
        let mut rx = relay_agents.subscribe();

        directory.set_visibility("scout", A2aVisibility::Private);

        let list = next_list(&mut rx).await;
        assert_eq!(list.first().map(|a| a.a2a_private), Some(true));
        relay_agents.stop();
    }

    #[tokio::test]
    async fn turning_the_a2a_listener_off_disables_every_agent() {
        let directory = directory(&[
            ("scout", A2aVisibility::Public),
            ("nova", A2aVisibility::Public),
        ]);
        let relay_agents = RelayAgents::spawn(directory, true);
        let mut rx = relay_agents.subscribe();

        relay_agents.set_a2a_listener_enabled(false);

        let disabled = next_list(&mut rx).await;
        assert_eq!(disabled.len(), 2);
        assert!(disabled.iter().all(|a| !a.a2a_enabled));

        relay_agents.set_a2a_listener_enabled(true);
        let enabled = next_list(&mut rx).await;
        assert!(enabled.iter().all(|a| a.a2a_enabled));
        relay_agents.stop();
    }

    #[tokio::test]
    async fn a_burst_of_changes_is_one_update() {
        let directory = directory(&[("scout", A2aVisibility::Public)]);
        let relay_agents = RelayAgents::spawn(Arc::clone(&directory) as _, true);
        let mut rx = relay_agents.subscribe();

        directory.add_agent("nova", A2aVisibility::Public, Router::new());
        directory.set_state("nova", AgentState::Starting);
        directory.set_state("nova", AgentState::Running);
        directory.set_visibility("nova", A2aVisibility::Private);

        let list = next_list(&mut rx).await;
        let nova = list.iter().find(|a| a.name == "nova").unwrap();
        assert!(
            nova.a2a_enabled && nova.a2a_private,
            "the update carries the final state"
        );
        no_update_within(&mut rx, SETTLE * 2).await;
        relay_agents.stop();
    }

    #[tokio::test]
    async fn events_that_do_not_change_the_list_send_nothing() {
        let directory = directory(&[("scout", A2aVisibility::Public)]);
        let relay_agents = RelayAgents::spawn(Arc::clone(&directory) as _, true);
        let mut rx = relay_agents.subscribe();

        // A state event that leaves the relay-facing description as it was.
        directory.set_state("scout", AgentState::Running);

        no_update_within(&mut rx, SETTLE * 3).await;
        relay_agents.stop();
    }
}
