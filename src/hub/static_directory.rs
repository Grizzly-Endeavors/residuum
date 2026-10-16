//! An [`AgentDirectory`] over a fixed set of A2A routers, for the hub's
//! tests.
//!
//! It wraps the A2A listener's [`StaticAgentDirectory`] for the lookups and
//! adds what the hub reads of a full directory: hub events and the set of
//! agents whose stop has begun. Changing an agent's state or visibility, or
//! adding or removing an agent, publishes the matching hub event. It refuses
//! everything else.

use std::collections::BTreeSet;
use std::sync::{PoisonError, RwLock};

use async_trait::async_trait;
use axum::Router;
use tokio::sync::broadcast;

use super::directory::{AgentDirectory, AgentFiles};
use super::types::{AgentActivity, AgentPatch, DeletedAgent, HubEvent, RestoreAgentRequest};
use crate::a2a::StaticAgentDirectory;
use crate::background::team::{
    A2aVisibility, Actor, AgentState, AgentSummary, CreateAgentRequest, DeleteOutcome,
    LifecycleError, TeamDirectory,
};

/// A directory of agents whose A2A routers are supplied up front, publishing
/// hub events as they change.
pub(crate) struct StaticHubDirectory {
    inner: StaticAgentDirectory,
    events: broadcast::Sender<HubEvent>,
    /// Set by [`Self::begin_stopping`], as a real stop request would from
    /// the moment it is made until the state changes.
    stopping: RwLock<BTreeSet<String>>,
}

impl Default for StaticHubDirectory {
    fn default() -> Self {
        Self {
            inner: StaticAgentDirectory::new(),
            events: broadcast::channel(64).0,
            stopping: RwLock::new(BTreeSet::new()),
        }
    }
}

impl StaticHubDirectory {
    /// An empty directory.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Add a running agent serving `router` (its A2A routes, rooted at `/`).
    /// Clears [`Self::begin_stopping`]'s flag for that name, as [`Self::add_agent`] does.
    #[must_use]
    pub(crate) fn with_agent(
        self,
        name: impl Into<String>,
        visibility: A2aVisibility,
        router: Router,
    ) -> Self {
        let name = name.into();
        self.stopping_set().remove(&name);
        Self {
            inner: self.inner.with_agent(name, visibility, router),
            ..self
        }
    }

    /// Move an agent to `state`, as a stop or failure would, and publish the
    /// change. Clears [`Self::begin_stopping`]'s flag, as a real state
    /// change does by replacing the agent's running handle.
    pub(crate) fn set_state(&self, name: &str, state: AgentState) {
        self.inner.set_state(name, state);
        self.stopping_set().remove(name);
        self.publish_state(name);
    }

    /// Mark a running agent as having begun stopping, as
    /// `AgentHost::stop_locked` does before the agent's event loop actually
    /// exits, and publish [`HubEvent::AgentStopping`].
    pub(crate) fn begin_stopping(&self, name: &str) {
        if self.inner.summary(name).is_err() {
            return;
        }
        self.stopping_set().insert(name.to_string());
        self.events
            .send(HubEvent::AgentStopping {
                name: name.to_string(),
            })
            .ok();
    }

    /// Change an agent's A2A visibility and publish the change.
    pub(crate) fn set_visibility(&self, name: &str, visibility: A2aVisibility) {
        self.inner.set_visibility(name, visibility);
        self.publish_state(name);
    }

    /// Add a running agent after construction and publish its creation.
    pub(crate) fn add_agent(&self, name: &str, visibility: A2aVisibility, router: Router) {
        self.inner.add_agent(name, visibility, router);
        self.stopping_set().remove(name);
        if let Ok(summary) = self.inner.summary(name) {
            self.events
                .send(HubEvent::AgentCreated {
                    agent: summary,
                    by: Actor::User,
                })
                .ok();
        }
    }

    /// Remove an agent and publish its deletion.
    pub(crate) fn remove_agent(&self, name: &str) {
        if self.inner.summary(name).is_err() {
            return;
        }
        self.inner.remove_agent(name);
        self.stopping_set().remove(name);
        self.events
            .send(HubEvent::AgentDeleted {
                name: name.to_string(),
                by: Actor::User,
            })
            .ok();
    }

    fn publish_state(&self, name: &str) {
        if let Ok(summary) = self.inner.summary(name) {
            self.events
                .send(HubEvent::AgentState { agent: summary })
                .ok();
        }
    }

    fn stopping_set(&self) -> std::sync::RwLockWriteGuard<'_, BTreeSet<String>> {
        self.stopping
            .write()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

fn unsupported(operation: &str) -> LifecycleError {
    LifecycleError::Failed(format!("{operation} isn't available on the A2A listener"))
}

#[async_trait]
impl TeamDirectory for StaticHubDirectory {
    fn list(&self) -> Vec<AgentSummary> {
        self.inner.list()
    }

    fn summary(&self, name: &str) -> Result<AgentSummary, LifecycleError> {
        self.inner.summary(name)
    }

    fn agent_a2a_router(&self, name: &str) -> Result<Router, LifecycleError> {
        self.inner.agent_a2a_router(name)
    }

    async fn create(
        &self,
        request: CreateAgentRequest,
        by: Actor,
    ) -> Result<AgentSummary, LifecycleError> {
        self.inner.create(request, by).await
    }

    async fn delete(&self, name: &str, by: Actor) -> Result<DeleteOutcome, LifecycleError> {
        self.inner.delete(name, by).await
    }
}

#[async_trait]
impl AgentDirectory for StaticHubDirectory {
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

    fn activity(&self) -> Vec<(String, AgentActivity)> {
        Vec::new()
    }

    fn stopping(&self) -> Vec<String> {
        self.stopping
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .cloned()
            .collect()
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
