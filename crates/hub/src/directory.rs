//! The interface to the set of hosted agents.
//!
//! The agent host implements it; the hub HTTP API and the hub WebSocket use
//! it, so each can be built and tested against a fake. Its supertrait,
//! [`TeamDirectory`], is the part the runtime uses (the team router, the A2A
//! listener, and agent tools that create or delete teammates).

use std::path::PathBuf;

use async_trait::async_trait;
use tokio::sync::broadcast;

use super::types::{AgentActivity, AgentPatch, DeletedAgent, HubEvent, RestoreAgentRequest};
use residuum_background::team::{Actor, AgentSummary, LifecycleError, TeamDirectory};
use residuum_bus::session::SessionInfo;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentFiles {
    pub dir: PathBuf,
    pub timezone: chrono_tz::Tz,
}

/// The hub's hosted agents: lookup, per-agent routing, and lifecycle.
#[async_trait]
pub trait AgentDirectory: TeamDirectory {
    /// The agent's HTTP router: today's agent-scoped routes (including its
    /// WebSocket at `/ws`), rooted at `/` with its state applied. The hub
    /// serves it under `/api/agents/{name}/` by stripping that prefix.
    ///
    /// # Errors
    /// [`LifecycleError::NotFound`] for an unknown agent;
    /// [`LifecycleError::NotRunning`] when it isn't running.
    fn agent_router(&self, name: &str) -> Result<axum::Router, LifecycleError>;

    fn agent_repair_router(&self, name: &str) -> Result<axum::Router, LifecycleError>;

    fn agent_file_router(&self, name: &str) -> Result<axum::Router, LifecycleError>;

    fn agent_files(&self, name: &str) -> Result<AgentFiles, LifecycleError>;

    fn activity(&self) -> Vec<(String, AgentActivity)>;

    fn stopping(&self) -> Vec<String> {
        Vec::new()
    }

    fn live_sessions(&self, _name: &str) -> Vec<SessionInfo> {
        Vec::new()
    }

    async fn list_deleted(&self) -> Result<Vec<DeletedAgent>, LifecycleError>;

    async fn restore(
        &self,
        request: RestoreAgentRequest,
        by: Actor,
    ) -> Result<AgentSummary, LifecycleError>;

    async fn start(&self, name: &str) -> Result<AgentSummary, LifecycleError>;

    async fn stop(&self, name: &str) -> Result<AgentSummary, LifecycleError>;

    async fn restart(&self, name: &str) -> Result<AgentSummary, LifecycleError>;

    async fn restart_now(&self, name: &str) -> Result<AgentSummary, LifecycleError> {
        self.restart(name).await
    }

    async fn patch(&self, name: &str, patch: AgentPatch) -> Result<AgentSummary, LifecycleError>;

    fn subscribe(&self) -> broadcast::Receiver<HubEvent>;
}
