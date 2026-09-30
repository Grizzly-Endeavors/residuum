//! An [`AgentDirectory`] over a fixed set of A2A routers.
//!
//! It answers what the A2A listener asks of a directory (`summary`,
//! `agent_a2a_router`) and refuses everything else, so the listener can serve
//! agents whose routers were built elsewhere, and be tested against agents
//! with chosen visibility and state. Changing an agent's state or visibility,
//! or adding or removing an agent, publishes the matching hub event.

use std::collections::BTreeMap;
use std::sync::RwLock;

use async_trait::async_trait;
use axum::Router;
use tokio::sync::broadcast;

use crate::hub::AgentDirectory;
use crate::hub::{
    A2aVisibility, Actor, AgentActivity, AgentFiles, AgentPatch, AgentState, AgentSummary,
    CreateAgentRequest, DeleteOutcome, DeletedAgent, HubEvent, LifecycleError, RestoreAgentRequest,
};

struct Entry {
    visibility: A2aVisibility,
    state: AgentState,
    /// Set by [`StaticAgentDirectory::begin_stopping`], as a real stop
    /// request would from the moment it is made until the state changes.
    stopping: bool,
    router: Router,
}

/// A directory of agents whose A2A routers are supplied up front.
pub struct StaticAgentDirectory {
    agents: RwLock<BTreeMap<String, Entry>>,
    events: broadcast::Sender<HubEvent>,
}

impl Default for StaticAgentDirectory {
    fn default() -> Self {
        Self {
            agents: RwLock::new(BTreeMap::new()),
            events: broadcast::channel(64).0,
        }
    }
}

impl StaticAgentDirectory {
    /// An empty directory.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a running agent serving `router` (its A2A routes, rooted at `/`).
    #[must_use]
    pub fn with_agent(
        self,
        name: impl Into<String>,
        visibility: A2aVisibility,
        router: Router,
    ) -> Self {
        self.agents
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                name.into(),
                Entry {
                    visibility,
                    state: AgentState::Running,
                    stopping: false,
                    router,
                },
            );
        self
    }

    /// Move an agent to `state`, as a stop or failure would, and publish the
    /// change. Clears [`Self::begin_stopping`]'s flag, as a real state
    /// change does by replacing the agent's running handle.
    #[cfg(test)]
    pub(crate) fn set_state(&self, name: &str, state: AgentState) {
        self.change(name, |entry| {
            entry.state = state;
            entry.stopping = false;
        });
    }

    /// Mark a running agent as having begun stopping, as
    /// `AgentHost::stop_locked` does before the agent's event loop actually
    /// exits, and publish [`HubEvent::AgentStopping`].
    #[cfg(test)]
    pub(crate) fn begin_stopping(&self, name: &str) {
        let found = {
            let mut agents = self
                .agents
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(entry) = agents.get_mut(name) else {
                return;
            };
            entry.stopping = true;
            true
        };
        if found {
            self.events
                .send(HubEvent::AgentStopping {
                    name: name.to_string(),
                })
                .ok();
        }
    }

    /// Change an agent's A2A visibility and publish the change.
    #[cfg(test)]
    pub(crate) fn set_visibility(&self, name: &str, visibility: A2aVisibility) {
        self.change(name, |entry| entry.visibility = visibility);
    }

    /// Add a running agent after construction and publish its creation.
    #[cfg(test)]
    pub(crate) fn add_agent(&self, name: &str, visibility: A2aVisibility, router: Router) {
        let summary = {
            let mut agents = self
                .agents
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let entry = Entry {
                visibility,
                state: AgentState::Running,
                stopping: false,
                router,
            };
            let summary = Self::summary_of(name, &entry);
            agents.insert(name.to_string(), entry);
            summary
        };
        self.events
            .send(HubEvent::AgentCreated {
                agent: summary,
                by: Actor::User,
            })
            .ok();
    }

    /// Remove an agent and publish its deletion.
    #[cfg(test)]
    pub(crate) fn remove_agent(&self, name: &str) {
        let removed = self
            .agents
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(name)
            .is_some();
        if removed {
            self.events
                .send(HubEvent::AgentDeleted {
                    name: name.to_string(),
                    by: Actor::User,
                })
                .ok();
        }
    }

    #[cfg(test)]
    fn change(&self, name: &str, apply: impl FnOnce(&mut Entry)) {
        let summary = {
            let mut agents = self
                .agents
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(entry) = agents.get_mut(name) else {
                return;
            };
            apply(entry);
            Self::summary_of(name, entry)
        };
        self.events
            .send(HubEvent::AgentState { agent: summary })
            .ok();
    }

    fn summary_of(name: &str, entry: &Entry) -> AgentSummary {
        AgentSummary {
            name: name.to_string(),
            state: entry.state,
            last_error: None,
            autostart: true,
            role: None,
            a2a_visibility: entry.visibility,
        }
    }
}

fn unsupported(operation: &str) -> LifecycleError {
    LifecycleError::Failed(format!("{operation} isn't available on the A2A listener"))
}

#[async_trait]
impl AgentDirectory for StaticAgentDirectory {
    fn list(&self) -> Vec<AgentSummary> {
        self.agents
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .map(|(name, entry)| Self::summary_of(name, entry))
            .collect()
    }

    fn summary(&self, name: &str) -> Result<AgentSummary, LifecycleError> {
        self.agents
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(name)
            .map(|entry| Self::summary_of(name, entry))
            .ok_or_else(|| LifecycleError::NotFound(name.to_string()))
    }

    fn agent_router(&self, _name: &str) -> Result<Router, LifecycleError> {
        Err(unsupported("the agent HTTP router"))
    }

    fn agent_repair_router(&self, _name: &str) -> Result<Router, LifecycleError> {
        Err(unsupported("the agent repair router"))
    }

    fn agent_file_router(&self, _name: &str) -> Result<Router, LifecycleError> {
        Err(unsupported("the agent file router"))
    }

    fn agent_files(&self, _name: &str) -> Result<AgentFiles, LifecycleError> {
        Err(unsupported("the agent's files"))
    }

    fn agent_a2a_router(&self, name: &str) -> Result<Router, LifecycleError> {
        let agents = self
            .agents
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = agents
            .get(name)
            .ok_or_else(|| LifecycleError::NotFound(name.to_string()))?;
        if entry.state == AgentState::Running {
            Ok(entry.router.clone())
        } else {
            Err(LifecycleError::NotRunning {
                name: name.to_string(),
                state: entry.state,
            })
        }
    }

    fn activity(&self) -> Vec<(String, AgentActivity)> {
        Vec::new()
    }

    fn stopping(&self) -> Vec<String> {
        self.agents
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|(_, entry)| entry.stopping)
            .map(|(name, _)| name.clone())
            .collect()
    }

    async fn create(
        &self,
        _request: CreateAgentRequest,
        _by: Actor,
    ) -> Result<AgentSummary, LifecycleError> {
        Err(unsupported("creating an agent"))
    }

    async fn delete(&self, _name: &str, _by: Actor) -> Result<DeleteOutcome, LifecycleError> {
        Err(unsupported("deleting an agent"))
    }

    async fn list_deleted(&self) -> Result<Vec<DeletedAgent>, LifecycleError> {
        Ok(Vec::new())
    }

    async fn restore(
        &self,
        _request: RestoreAgentRequest,
        _by: Actor,
    ) -> Result<AgentSummary, LifecycleError> {
        Err(unsupported("restoring an agent"))
    }

    async fn start(&self, _name: &str) -> Result<AgentSummary, LifecycleError> {
        Err(unsupported("starting an agent"))
    }

    async fn stop(&self, _name: &str) -> Result<AgentSummary, LifecycleError> {
        Err(unsupported("stopping an agent"))
    }

    async fn restart(&self, _name: &str) -> Result<AgentSummary, LifecycleError> {
        Err(unsupported("restarting an agent"))
    }

    async fn patch(&self, _name: &str, _patch: AgentPatch) -> Result<AgentSummary, LifecycleError> {
        Err(unsupported("changing an agent"))
    }

    fn subscribe(&self) -> broadcast::Receiver<HubEvent> {
        self.events.subscribe()
    }
}
