use std::path::PathBuf;
use std::sync::Arc;

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use serde_json::json;

use crate::file_server::{self, FileRegistry};
use residuum_bus::control::ReloadSender;
use residuum_checkpoints::{
    CheckpointContext, CheckpointEngine, CheckpointError, CheckpointTrigger,
};

use super::{ConfigApiState, a2a, chat, context, inbox};

type OpenCheckpoints = dyn Fn() -> Result<Arc<CheckpointEngine>, CheckpointError> + Send + Sync;

#[derive(Clone)]
pub enum CheckpointAccess {
    Open(Arc<CheckpointEngine>),
    Lazy(Arc<OpenCheckpoints>),
}

impl CheckpointAccess {
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

#[derive(Clone)]
pub struct AgentFilesState {
    pub agent_name: String,
    pub workspace_dir: PathBuf,
    pub memory_dir: Option<PathBuf>,
    pub reload_tx: Option<ReloadSender>,
    pub checkpoints: CheckpointAccess,
    pub file_registry: FileRegistry,
}

impl AgentFilesState {
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

    #[cfg(test)]
    pub(crate) fn for_test(workspace_dir: impl Into<PathBuf>, memory_dir: Option<PathBuf>) -> Self {
        let workspace_dir = workspace_dir.into();
        Self {
            agent_name: "test-agent".to_string(),
            file_registry: FileRegistry::for_agent(
                "test-agent",
                &residuum_config::WorkspaceLayout::new(&workspace_dir),
            ),
            workspace_dir,
            memory_dir,
            reload_tx: None,
            checkpoints: CheckpointAccess::Open(residuum_checkpoints::testing::test_engine()),
        }
    }

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

pub fn agent_files_api_router(state: AgentFilesState) -> axum::Router {
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
        .route("/api/context", get(context::api_context))
        .route("/api/a2a/agents/raw", get(a2a::api_a2a_agents_raw_get))
        .route("/api/a2a/agents/raw", put(a2a::api_a2a_agents_raw_put))
        .route(
            "/api/inbox/{id}/attachments/{index}",
            get(inbox::api_inbox_attachment),
        )
        .route(
            "/api/inbox/agent/{id}/attachments/{index}",
            get(inbox::api_agent_inbox_attachment),
        )
        .with_state(state)
        .merge(file_links)
}

#[derive(Debug)]
pub(super) struct FileRouteError {
    status: StatusCode,
    message: String,
}

impl FileRouteError {
    pub(super) fn bad_request(message: String) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message,
        }
    }

    pub(super) fn not_found(message: &str) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: message.to_string(),
        }
    }

    pub(super) fn internal(message: &str) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: message.to_string(),
        }
    }
}

impl IntoResponse for FileRouteError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.message }))).into_response()
    }
}
