use async_trait::async_trait;

use super::types::{Actor, AgentSummary, CreateAgentRequest, DeleteOutcome, LifecycleError};

#[async_trait]
pub trait TeamDirectory: Send + Sync {
    fn list(&self) -> Vec<AgentSummary>;

    fn summary(&self, name: &str) -> Result<AgentSummary, LifecycleError>;

    /// The agent's A2A server router (card, JSON-RPC, REST), rooted at `/`.
    /// The A2A listener serves it under `/agents/{name}/`.
    ///
    /// # Errors
    /// [`LifecycleError::NotFound`] for an unknown agent;
    /// [`LifecycleError::NotRunning`] when it isn't running.
    fn agent_a2a_router(&self, name: &str) -> Result<axum::Router, LifecycleError>;

    async fn create(
        &self,
        request: CreateAgentRequest,
        by: Actor,
    ) -> Result<AgentSummary, LifecycleError>;

    async fn delete(&self, name: &str, by: Actor) -> Result<DeleteOutcome, LifecycleError>;
}

pub type DirectoryHandle = residuum_core::util::LateBound<dyn TeamDirectory>;
