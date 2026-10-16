//! A [`TeamDirectory`] over a fixed set of A2A routers.
//!
//! It answers what the A2A listener asks of a directory (`summary`,
//! `agent_a2a_router`) and refuses creating and deleting agents, so the
//! listener can serve agents whose routers were built elsewhere, and be
//! tested against agents with chosen visibility and state.

use std::collections::BTreeMap;
use std::sync::RwLock;

use async_trait::async_trait;
use axum::Router;

use crate::background::team::{
    A2aVisibility, Actor, AgentState, AgentSummary, CreateAgentRequest, DeleteOutcome,
    LifecycleError, TeamDirectory,
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
        self.add_agent(&name.into(), visibility, router);
        self
    }

    /// Move an agent to `state`, as a stop or failure would.
    pub(crate) fn set_state(&self, name: &str, state: AgentState) {
        self.change(name, |entry| entry.state = state);
    }

    /// Change an agent's A2A visibility.
    pub(crate) fn set_visibility(&self, name: &str, visibility: A2aVisibility) {
        self.change(name, |entry| entry.visibility = visibility);
    }

    /// Add a running agent after construction, replacing one of the same
    /// name.
    pub(crate) fn add_agent(&self, name: &str, visibility: A2aVisibility, router: Router) {
        self.agents
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                name.to_string(),
                Entry {
                    visibility,
                    state: AgentState::Running,
                    router,
                },
            );
    }

    /// Remove an agent.
    pub(crate) fn remove_agent(&self, name: &str) {
        self.agents
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(name);
    }

    fn change(&self, name: &str, apply: impl FnOnce(&mut Entry)) {
        if let Some(entry) = self
            .agents
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_mut(name)
        {
            apply(entry);
        }
    }

    fn summary_of(name: &str, entry: &Entry) -> AgentSummary {
        AgentSummary {
            name: name.to_string(),
            display_name: name.to_string(),
            state: entry.state,
            last_error: None,
            autostart: true,
            role: None,
            a2a_visibility: entry.visibility,
            teams_configured: false,
        }
    }
}

fn unsupported(operation: &str) -> LifecycleError {
    LifecycleError::Failed(format!("{operation} isn't available on the A2A listener"))
}

#[async_trait]
impl TeamDirectory for StaticAgentDirectory {
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
}
