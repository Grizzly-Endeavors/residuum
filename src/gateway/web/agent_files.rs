//! The per-agent routes that only read and write the agent's files (chat
//! history and usage, the user inbox, the raw A2A client settings, and the
//! file links: workspace links and token links), and the state they run on.
//!
//! The state holds no open checkpoint repositories. The one handler that
//! takes a checkpoint, the raw A2A settings write, opens them when it runs,
//! so a history, usage or inbox request never touches them and keeps working
//! when they can't be opened.

use std::path::PathBuf;
use std::sync::Arc;

use axum::routing::{get, post, put};

use crate::checkpoints::{CheckpointContext, CheckpointEngine, CheckpointError, CheckpointTrigger};
use crate::gateway::file_server::{self, FileRegistry};
use crate::gateway::types::ReloadSender;

use super::{ConfigApiState, a2a, chat, inbox};

/// Opens an agent's checkpoint repositories.
type OpenCheckpoints = dyn Fn() -> Result<Arc<CheckpointEngine>, CheckpointError> + Send + Sync;

/// How a handler gets the agent's checkpoint repositories.
#[derive(Clone)]
pub enum CheckpointAccess {
    /// An engine that is already open, as a running agent holds.
    Open(Arc<CheckpointEngine>),
    /// Opened when a handler asks for them, and not before.
    Lazy(Arc<OpenCheckpoints>),
}

impl CheckpointAccess {
    /// Open the repositories with `open` each time a handler asks for them.
    pub fn lazy(
        open: impl Fn() -> Result<Arc<CheckpointEngine>, CheckpointError> + Send + Sync + 'static,
    ) -> Self {
        Self::Lazy(Arc::new(open))
    }

    fn engine(&self) -> Result<Arc<CheckpointEngine>, CheckpointError> {
        match self {
            Self::Open(engine) => Ok(Arc::clone(engine)),
            Self::Lazy(open) => open(),
        }
    }
}

/// Shared state for the file-only agent routes.
#[derive(Clone)]
pub struct AgentFilesState {
    /// This agent's name, used in the attachment URLs an inbox listing hands
    /// out and in the app address a workspace file link that can't be served
    /// sends a browser to.
    pub agent_name: String,
    /// Path to the workspace root directory.
    pub workspace_dir: PathBuf,
    /// Path to the workspace memory directory (None in setup mode).
    pub memory_dir: Option<PathBuf>,
    /// Signal the running agent to reload (None when there is no live agent
    /// to signal).
    pub reload_tx: Option<ReloadSender>,
    /// The agent's checkpoint repositories.
    pub checkpoints: CheckpointAccess,
    /// The registry the file links are served from: the running agent's
    /// own, or one over the links it saved while it is stopped
    /// ([`FileRegistry::for_agent`]).
    pub file_registry: FileRegistry,
}

impl AgentFilesState {
    /// The state of a running agent, over its open state and its live file
    /// registry.
    #[must_use]
    pub fn running(state: &ConfigApiState, file_registry: FileRegistry) -> Self {
        Self {
            agent_name: state.agent_name.clone(),
            workspace_dir: state.workspace_dir.clone(),
            memory_dir: state.memory_dir.clone(),
            reload_tx: state.reload_tx.clone(),
            checkpoints: CheckpointAccess::Open(Arc::clone(&state.checkpoints)),
            file_registry,
        }
    }

    /// A state over `workspace_dir` with throwaway checkpoint repositories
    /// and no one listening for reloads.
    #[cfg(test)]
    pub(crate) fn for_test(workspace_dir: impl Into<PathBuf>, memory_dir: Option<PathBuf>) -> Self {
        let workspace_dir = workspace_dir.into();
        Self {
            agent_name: "test-agent".to_string(),
            file_registry: FileRegistry::for_agent(
                "test-agent",
                &crate::workspace::layout::WorkspaceLayout::new(&workspace_dir),
            ),
            workspace_dir,
            memory_dir,
            reload_tx: None,
            checkpoints: CheckpointAccess::Open(crate::checkpoints::test_engine()),
        }
    }

    /// Checkpoint the workspace repository before a raw write to a
    /// workspace-owned strictly-parsed file. Never fails or blocks the
    /// write, as with every other checkpoint: repositories that can't be
    /// opened are logged and the write goes ahead without one.
    pub(super) async fn checkpoint_workspace_before_write(&self, summary: impl Into<String>) {
        let engine = match self.checkpoints.engine() {
            Ok(engine) => engine,
            Err(e) => {
                tracing::warn!(error = %e, "couldn't open the agent's checkpoint repositories; saving without a checkpoint");
                return;
            }
        };
        let _checkpoint_id = engine
            .checkpoint_workspace_id_before_action(CheckpointContext::system(
                CheckpointTrigger::PreAction,
                summary,
            ))
            .await;
    }
}

/// The routes that read and write an agent's files and nothing of its live
/// state: chat history, session usage, the user inbox, the raw A2A client
/// settings, and the file links the agent hands out.
pub fn agent_files_api_router(state: AgentFilesState) -> axum::Router {
    // A workspace link serves a file under the workspace root by its path,
    // and a token link the file its saved registration names, so both answer
    // from the registry whether or not the agent is running.
    let file_links: axum::Router = axum::Router::new()
        .route(
            "/api/files/workspace",
            get(file_server::serve_workspace_file),
        )
        .route("/api/files/{id}", get(file_server::serve_file))
        .with_state(state.file_registry.clone());
    axum::Router::new()
        .route("/api/chat/history", get(chat::api_chat_history))
        .route("/api/usage", get(chat::api_usage))
        .route("/api/a2a/agents/raw", get(a2a::api_a2a_agents_raw_get))
        .route("/api/a2a/agents/raw", put(a2a::api_a2a_agents_raw_put))
        .route("/api/inbox", get(inbox::api_inbox_list))
        .route("/api/inbox/archive", get(inbox::api_inbox_archive_list))
        .route("/api/inbox/{id}/read", put(inbox::api_inbox_read))
        .route("/api/inbox/{id}/archive", post(inbox::api_inbox_archive))
        .route("/api/inbox/{id}/restore", post(inbox::api_inbox_restore))
        .route(
            "/api/inbox/{id}/attachments/{index}",
            get(inbox::api_inbox_attachment),
        )
        .with_state(state)
        .merge(file_links)
}
