//! The team's view of the hub's hosted agents.
//!
//! [`TeamDirectory`] is what the runtime asks of the hub: the team router
//! reads the roster and each agent's state, the A2A listener finds an
//! agent's A2A router, and the agent lifecycle tools create and delete
//! teammates. The agent host implements it, and the hub's `AgentDirectory`
//! extends it with the routes, lifecycle and events only the hub needs, so
//! each user can be built and tested against a fake of this trait.

use std::sync::{Arc, OnceLock, Weak};

use async_trait::async_trait;

use super::types::{Actor, AgentSummary, CreateAgentRequest, DeleteOutcome, LifecycleError};

/// The hub's hosted agents as the team sees them: lookup, each agent's A2A
/// routes, and creating and deleting agents.
#[async_trait]
pub trait TeamDirectory: Send + Sync {
    /// Every agent, sorted by name.
    fn list(&self) -> Vec<AgentSummary>;

    /// One agent's summary.
    ///
    /// # Errors
    /// [`LifecycleError::NotFound`] when no agent has this name.
    fn summary(&self, name: &str) -> Result<AgentSummary, LifecycleError>;

    /// The agent's A2A server router (card, JSON-RPC, REST), rooted at `/`.
    /// The A2A listener serves it under `/agents/{name}/`.
    ///
    /// # Errors
    /// [`LifecycleError::NotFound`] for an unknown agent;
    /// [`LifecycleError::NotRunning`] when it isn't running.
    fn agent_a2a_router(&self, name: &str) -> Result<axum::Router, LifecycleError>;

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
}

/// A late-bound, weak reference to the hub's [`TeamDirectory`].
///
/// The host is built after the shared services every agent receives, and
/// each agent's tools are built inside a host-started agent, so the services
/// carry this handle and the host binds itself into it on construction. It
/// is weak because the agents hold it and the host holds the agents; a
/// strong reference would keep the host alive from inside its own agents.
#[derive(Clone, Default)]
pub struct DirectoryHandle {
    inner: Arc<OnceLock<Weak<dyn TeamDirectory>>>,
}

impl DirectoryHandle {
    /// A handle that points at nothing until [`Self::bind`] is called.
    #[must_use]
    pub fn unbound() -> Self {
        Self::default()
    }

    /// Point the handle at `directory`. A handle binds once; later calls
    /// leave the first binding in place.
    pub fn bind(&self, directory: Weak<dyn TeamDirectory>) {
        self.inner.set(directory).ok();
    }

    /// The directory, when it is bound and still alive (the hub is not
    /// shutting down).
    #[must_use]
    pub fn get(&self) -> Option<Arc<dyn TeamDirectory>> {
        self.inner.get().and_then(Weak::upgrade)
    }
}
