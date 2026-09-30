//! An [`AgentDirectory`] over a fixed set of A2A routers.
//!
//! It answers what the A2A listener asks of a directory (`summary`,
//! `agent_a2a_router`) and refuses everything else, so the listener can serve
//! agents whose routers were built elsewhere, and be tested against agents
//! with chosen visibility and state.

use std::collections::BTreeMap;
use std::sync::RwLock;

use async_trait::async_trait;
use axum::Router;
use tokio::sync::broadcast;

use crate::hub::AgentDirectory;
use crate::hub::{
    A2aVisibility, Actor, AgentActivity, AgentPatch, AgentState, AgentSummary, CreateAgentRequest,
    DeleteOutcome, HubEvent, LifecycleError,
};

struct Entry {
    visibility: A2aVisibility,
    state: AgentState,
    router: Router,
}

/// A directory of agents whose A2A routers are supplied up front.
#[derive(Default)]
pub struct StaticAgentDirectory {
    agents: RwLock<BTreeMap<String, Entry>>,
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
                    router,
                },
            );
        self
    }

    /// Move an agent to `state`, as a stop or failure would.
    #[cfg(test)]
    pub(crate) fn set_state(&self, name: &str, state: AgentState) {
        if let Some(entry) = self
            .agents
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_mut(name)
        {
            entry.state = state;
        }
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
        broadcast::channel(1).1
    }
}
