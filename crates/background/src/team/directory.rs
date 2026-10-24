//! The team's view of the hub's hosted agents.
//!
//! [`TeamDirectory`] is what the runtime asks of the hub: the team router
//! reads the roster and each agent's state, the A2A listener finds an
//! agent's A2A router, and the agent lifecycle tools create and delete
//! teammates. The agent host implements it, and the hub's `AgentDirectory`
//! extends it with the routes, lifecycle and events only the hub needs, so
//! each user can be built and tested against a fake of this trait.

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

/// A late-bound, weak reference to the hub's [`TeamDirectory`], which the
/// agent host binds itself into on construction.
pub type DirectoryHandle = residuum_core::util::LateBound<dyn TeamDirectory>;
