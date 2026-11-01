//! `GET /api/agents/{name}/context`: what the agent's next model request is
//! made of, as the agent saved it after its latest turn or memory change.

use axum::Json;
use axum::extract::State;

use residuum_agent::context::{ContextSnapshot, load_context_snapshot};
use residuum_config::WorkspaceLayout;

use super::agent_files::{AgentFilesState, FileRouteError};

/// `GET /api/agents/{name}/context` — the context breakdown the agent saved
/// after its latest turn or memory change (`memory/context_breakdown.json`),
/// with when it was computed. It reads the file, not the live agent, so it
/// answers for a stopped agent too and never waits for a running turn. A
/// `404` means no turn has saved one yet; a file that can't be read or parsed
/// is a `500`, logged.
///
/// # Errors
/// `404` when there is no breakdown yet, `500` when it can't be read.
pub(super) async fn api_context(
    State(state): State<AgentFilesState>,
) -> Result<Json<ContextSnapshot>, FileRouteError> {
    let path = WorkspaceLayout::new(&state.workspace_dir).context_breakdown_json();
    match load_context_snapshot(&path).await {
        Ok(Some(snapshot)) => Ok(Json(snapshot)),
        Ok(None) => Err(FileRouteError::not_found(
            "no context breakdown yet; the agent saves one after its next turn",
        )),
        Err(e) => {
            tracing::error!(error = ?e, path = %path.display(), "failed to read the context breakdown");
            Err(FileRouteError::internal(
                "couldn't read the context breakdown; the details are in Residuum's logs",
            ))
        }
    }
}
