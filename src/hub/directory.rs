//! The interface to the set of hosted agents.
//!
//! The agent host implements it; the hub HTTP API, the hub WebSocket, the
//! A2A listener, and agent tools that create or delete teammates use it, so
//! each can be built and tested against a fake.

use std::sync::{Arc, OnceLock, Weak};

use async_trait::async_trait;
use tokio::sync::broadcast;

use super::types::{
    Actor, AgentActivity, AgentPatch, AgentSummary, CreateAgentRequest, DeleteOutcome,
    DeletedAgent, HubEvent, LifecycleError, RestoreAgentRequest,
};

/// The hub's hosted agents: lookup, per-agent routing, and lifecycle.
#[async_trait]
pub trait AgentDirectory: Send + Sync {
    /// Every agent, sorted by name.
    fn list(&self) -> Vec<AgentSummary>;

    /// One agent's summary.
    ///
    /// # Errors
    /// [`LifecycleError::NotFound`] when no agent has this name.
    fn summary(&self, name: &str) -> Result<AgentSummary, LifecycleError>;

    /// The agent's HTTP router: today's agent-scoped routes (including its
    /// WebSocket at `/ws`), rooted at `/` with its state applied. The hub
    /// serves it under `/api/agents/{name}/` by stripping that prefix.
    ///
    /// # Errors
    /// [`LifecycleError::NotFound`] for an unknown agent;
    /// [`LifecycleError::NotRunning`] when it isn't running.
    fn agent_router(&self, name: &str) -> Result<axum::Router, LifecycleError>;

    /// The agent's config, file, and checkpoint routes, which also work on a
    /// stopped or failed agent so the user can repair it. Same rooting as
    /// [`Self::agent_router`].
    ///
    /// # Errors
    /// [`LifecycleError::NotFound`] for an unknown agent.
    fn agent_repair_router(&self, name: &str) -> Result<axum::Router, LifecycleError>;

    /// The agent's A2A server router (card, JSON-RPC, REST), rooted at `/`.
    /// The A2A listener serves it under `/agents/{name}/`.
    ///
    /// # Errors
    /// [`LifecycleError::NotFound`] for an unknown agent;
    /// [`LifecycleError::NotRunning`] when it isn't running.
    fn agent_a2a_router(&self, name: &str) -> Result<axum::Router, LifecycleError>;

    /// Main-conversation activity for every agent.
    fn activity(&self) -> Vec<(String, AgentActivity)>;

    /// Create an agent (see the design's creation order) and start it.
    ///
    /// # Errors
    /// Name, request, or existence problems, or a failure writing or
    /// starting the agent.
    async fn create(
        &self,
        request: CreateAgentRequest,
        by: Actor,
    ) -> Result<AgentSummary, LifecycleError>;

    /// Stop, checkpoint, and remove an agent and its role page.
    ///
    /// # Errors
    /// [`LifecycleError::NotFound`], or a failure removing it.
    async fn delete(&self, name: &str, by: Actor) -> Result<DeleteOutcome, LifecycleError>;

    /// The agents that were deleted and still have checkpoint history, by
    /// name, each with when it was deleted and the checkpoint a restore uses
    /// by default. An agent that exists, or was restored, is not listed.
    ///
    /// # Errors
    /// [`LifecycleError::Failed`] when the checkpoint history can't be read.
    async fn list_deleted(&self) -> Result<Vec<DeletedAgent>, LifecycleError>;

    /// Bring a deleted agent back from its checkpoint history and start it
    /// when its settings say to.
    ///
    /// # Errors
    /// [`LifecycleError::AlreadyExists`] when the name is taken;
    /// [`LifecycleError::NoDeletedAgent`] when there is no history to
    /// restore from; or a failure restoring or starting it.
    async fn restore(
        &self,
        request: RestoreAgentRequest,
        by: Actor,
    ) -> Result<AgentSummary, LifecycleError>;

    /// Start a stopped or failed agent.
    ///
    /// # Errors
    /// [`LifecycleError::NotFound`], or the start-up failure.
    async fn start(&self, name: &str) -> Result<AgentSummary, LifecycleError>;

    /// Stop a running agent: its adapters and event loop end, and its live
    /// sessions are recorded as interrupted.
    ///
    /// # Errors
    /// [`LifecycleError::NotFound`], or a failure stopping it.
    async fn stop(&self, name: &str) -> Result<AgentSummary, LifecycleError>;

    /// Stop then start an agent.
    ///
    /// # Errors
    /// As [`Self::stop`] and [`Self::start`].
    async fn restart(&self, name: &str) -> Result<AgentSummary, LifecycleError>;

    /// Change autostart and/or A2A visibility (writes the agent's config).
    ///
    /// # Errors
    /// [`LifecycleError::NotFound`]; [`LifecycleError::InvalidRequest`] for
    /// an empty patch; or a failure writing the config.
    async fn patch(&self, name: &str, patch: AgentPatch) -> Result<AgentSummary, LifecycleError>;

    /// Subscribe to hub events (state changes, created, deleted, activity,
    /// notices), in publish order.
    fn subscribe(&self) -> broadcast::Receiver<HubEvent>;
}

/// A late-bound, weak reference to the hub's [`AgentDirectory`].
///
/// The host is built after the shared services every agent receives, and
/// each agent's tools are built inside a host-started agent, so the services
/// carry this handle and the host binds itself into it on construction. It
/// is weak because the agents hold it and the host holds the agents; a
/// strong reference would keep the host alive from inside its own agents.
#[derive(Clone, Default)]
pub struct DirectoryHandle {
    inner: Arc<OnceLock<Weak<dyn AgentDirectory>>>,
}

impl DirectoryHandle {
    /// A handle that points at nothing until [`Self::bind`] is called.
    #[must_use]
    pub fn unbound() -> Self {
        Self::default()
    }

    /// Point the handle at `directory`. A handle binds once; later calls
    /// leave the first binding in place.
    pub fn bind(&self, directory: Weak<dyn AgentDirectory>) {
        self.inner.set(directory).ok();
    }

    /// The directory, when it is bound and still alive (the hub is not
    /// shutting down).
    #[must_use]
    pub fn get(&self) -> Option<Arc<dyn AgentDirectory>> {
        self.inner.get().and_then(Weak::upgrade)
    }
}
