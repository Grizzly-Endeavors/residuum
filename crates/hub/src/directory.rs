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

/// Where one agent's files are, and the timezone the hub reads the naive local
/// times stored in them in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentFiles {
    /// The agent's directory (`~/.residuum/<name>`), whatever its state.
    pub dir: PathBuf,
    /// The hub's configured timezone now. Times the agent's files keep carry
    /// no offset, so this is what makes them instants (see
    /// [`residuum_core::time::local_to_instant`]).
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

    /// The agent's config, file, and checkpoint routes, which also work on a
    /// stopped or failed agent so the user can repair it. Same rooting as
    /// [`Self::agent_router`].
    ///
    /// # Errors
    /// [`LifecycleError::NotFound`] for an unknown agent.
    fn agent_repair_router(&self, name: &str) -> Result<axum::Router, LifecycleError>;

    /// The agent's routes that only read and write its files (chat history,
    /// usage, the inbox attachments, the raw A2A settings, and the workspace file
    /// links), over its files on disk and with no checkpoint repository
    /// opened until a write needs one. The hub serves them from
    /// [`Self::agent_router`] while the agent is running, and from this
    /// router otherwise. Same rooting as [`Self::agent_router`].
    ///
    /// # Errors
    /// [`LifecycleError::NotFound`] for an unknown agent.
    fn agent_file_router(&self, name: &str) -> Result<axum::Router, LifecycleError>;

    /// Where the agent's files are, for what the hub reads across agents
    /// without going through an agent's own routes (mail and the unread counts).
    ///
    /// # Errors
    /// [`LifecycleError::NotFound`] for an unknown agent.
    fn agent_files(&self, name: &str) -> Result<AgentFiles, LifecycleError>;

    /// Main-conversation activity for every agent.
    fn activity(&self) -> Vec<(String, AgentActivity)>;

    /// Names of agents whose stop has begun but not finished: `list()` and
    /// `summary()` still report them `Running`, but the team router already
    /// refuses teammate messages for them from this point (see
    /// [`HubEvent::AgentStopping`]). Defaults to none, for directories with
    /// no such window.
    fn stopping(&self) -> Vec<String> {
        Vec::new()
    }

    /// The session runs going on in the running agent `name`, oldest first.
    /// Empty when the agent isn't running, or the directory has no sessions
    /// to report.
    fn live_sessions(&self, _name: &str) -> Vec<SessionInfo> {
        Vec::new()
    }

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
