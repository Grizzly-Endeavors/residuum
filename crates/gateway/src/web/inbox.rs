//! The agent inbox's one write endpoint for workbench artifacts
//! (`POST /api/agents/{name}/agent-inbox`), served by the running agent.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Json;
use serde::{Deserialize, Serialize};

use crate::types::GatewayState;

/// Build the agent-inbox API router (`POST /api/agents/{name}/agent-inbox`).
///
/// A workbench artifact's only way to hand the agent something to triage
/// later — there is no equivalent write endpoint for the user inbox, which
/// only the agent's `user_inbox_add` tool may populate.
pub(crate) fn agent_inbox_api_router(state: GatewayState) -> axum::Router {
    axum::Router::new()
        .route("/api/agent-inbox", axum::routing::post(api_agent_inbox_add))
        .with_state(state)
}

/// Request body for `POST /api/agents/{name}/agent-inbox`.
#[derive(Debug, Deserialize)]
pub(super) struct AgentInboxAddRequest {
    /// Defaults to the body's first line, in full, when absent or blank.
    #[serde(default)]
    pub title: Option<String>,
    pub body: String,
}

/// Response body for `POST /api/agents/{name}/agent-inbox`.
#[derive(Debug, Serialize)]
pub(super) struct AgentInboxAddResponse {
    /// The new item's ID (its filename stem), as used by `inbox_read` and
    /// `inbox_archive`.
    pub id: String,
}

/// `POST /api/agents/{name}/agent-inbox` — add an item to the agent's inbox, the same
/// place the WS `/inbox` command and the notification router's `inbox`
/// target write to. The source is `artifact:<name>` when the request carries
/// the artifact identity header (set by the workbench SDK), `"web"`
/// otherwise.
///
/// # Errors
/// `400` for a blank body or an invalid identity header, `500` if the item
/// can't be saved.
pub(super) async fn api_agent_inbox_add(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(req): Json<AgentInboxAddRequest>,
) -> Result<Json<AgentInboxAddResponse>, (StatusCode, String)> {
    if req.body.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "body must not be blank".to_string(),
        ));
    }

    let title = req
        .title
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| residuum_workspace::inbox::derive_title(&req.body));
    let source = match super::artifact_identity::artifact_identity(&headers) {
        Ok(Some(name)) => format!("artifact:{name}"),
        Ok(None) => "web".to_string(),
        Err(message) => return Err((StatusCode::BAD_REQUEST, message)),
    };

    let filename = residuum_workspace::inbox::quick_add(
        &state.agent_inbox_dir,
        &title,
        &req.body,
        &source,
        state.tz,
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, source = %source, "failed to add agent inbox item via http");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("couldn't save the inbox item: {e}"),
        )
    })?;

    let id = filename.trim_end_matches(".json").to_string();
    Ok(Json(AgentInboxAddResponse { id }))
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "test code uses indexing for clarity"
)]
mod tests {
    use super::*;

    /// A minimal but real `GatewayState`, for exercising the agent-inbox
    /// handler directly rather than through the full HTTP stack.
    fn make_gateway_state(workspace_dir: &std::path::Path) -> GatewayState {
        let (core, _receivers) = crate::types::GatewayCore::new(
            workspace_dir.to_path_buf(),
            workspace_dir.to_path_buf(),
        );
        let session_registry =
            std::sync::Arc::new(residuum_background::registry::SessionRegistry::new());
        let session_store = std::sync::Arc::new(residuum_background::store::SessionStore::new(
            workspace_dir.join("sessions"),
        ));
        let agent_messenger =
            std::sync::Arc::new(residuum_background::messaging::AgentMessenger::new(
                std::sync::Arc::clone(&session_registry),
                core.publisher.clone(),
                std::sync::Arc::clone(&session_store),
                residuum_agent::hop::HopLimits { soft: 8, hard: 32 },
            ));

        let agent_inbox_dir = workspace_dir.join("inbox/agent");
        std::fs::create_dir_all(&agent_inbox_dir).unwrap();

        GatewayState {
            turn_journal: crate::turn_journal::TurnJournal::default(),
            pulse_fires: std::sync::Arc::default(),
            queued: std::sync::Arc::default(),
            reload_tx: core.reload_tx,
            command_tx: core.command_tx,
            stop_tx: core.stop_tx,
            agent_inbox_dir,
            tz: chrono_tz::UTC,
            publisher: core.publisher,
            bus_handle: core.bus_handle,
            file_registry: residuum_hub::file_server::FileRegistry::new("scout"),
            webhooks: residuum_interfaces::webhook::WebhookTable::default(),
            session_registry,
            session_store,
            agent_messenger,
            skill_state: residuum_skills::SkillState::new_shared(
                residuum_skills::SkillIndex::default(),
                vec![],
            ),
            workspace_watch_health: tokio::sync::watch::channel(
                residuum_workspace::watch::WatchHealth::Native,
            )
            .1,
            team_feed: std::sync::Arc::new(residuum_hub::services::TeamChangeFeed::idle()),
            action_store: std::sync::Arc::new(tokio::sync::Mutex::new(
                residuum_proactivity::actions::store::ActionStore::new_empty(
                    workspace_dir.join("scheduled_actions.json"),
                ),
            )),
            layout: residuum_config::WorkspaceLayout::new(workspace_dir),
            activity: residuum_hub::activity::ActivityTracker::new(
                "test-agent",
                tokio::sync::broadcast::channel(4).0,
                residuum_hub::agent_watch::AgentChangeFeed::new(),
            ),
        }
    }

    #[tokio::test]
    async fn agent_inbox_add_defaults_title_and_web_source() {
        let dir = tempfile::tempdir().unwrap();
        let state = make_gateway_state(dir.path());

        let response = api_agent_inbox_add(
            State(state.clone()),
            HeaderMap::new(),
            Json(AgentInboxAddRequest {
                title: None,
                body: "First line of the item\nmore detail".to_string(),
            }),
        )
        .await
        .unwrap();

        let items = residuum_workspace::inbox::list_items(&state.agent_inbox_dir)
            .await
            .unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].0, response.0.id);
        assert_eq!(items[0].1.title, "First line of the item");
        assert_eq!(items[0].1.source, "web");
    }

    #[tokio::test]
    async fn agent_inbox_add_uses_given_title() {
        let dir = tempfile::tempdir().unwrap();
        let state = make_gateway_state(dir.path());

        let _response = api_agent_inbox_add(
            State(state.clone()),
            HeaderMap::new(),
            Json(AgentInboxAddRequest {
                title: Some("Custom title".to_string()),
                body: "body text".to_string(),
            }),
        )
        .await
        .unwrap();

        let items = residuum_workspace::inbox::list_items(&state.agent_inbox_dir)
            .await
            .unwrap();
        assert_eq!(items[0].1.title, "Custom title");
    }

    #[tokio::test]
    async fn agent_inbox_add_attributes_artifact_source() {
        let dir = tempfile::tempdir().unwrap();
        let state = make_gateway_state(dir.path());
        let mut headers = HeaderMap::new();
        headers.insert(
            super::super::artifact_identity::ARTIFACT_HEADER,
            "pricing-explorer".parse().unwrap(),
        );

        let _response = api_agent_inbox_add(
            State(state.clone()),
            headers,
            Json(AgentInboxAddRequest {
                title: None,
                body: "body text".to_string(),
            }),
        )
        .await
        .unwrap();

        let items = residuum_workspace::inbox::list_items(&state.agent_inbox_dir)
            .await
            .unwrap();
        assert_eq!(items[0].1.source, "artifact:pricing-explorer");
    }

    #[tokio::test]
    async fn agent_inbox_add_rejects_blank_body() {
        let dir = tempfile::tempdir().unwrap();
        let state = make_gateway_state(dir.path());

        let err = api_agent_inbox_add(
            State(state),
            HeaderMap::new(),
            Json(AgentInboxAddRequest {
                title: None,
                body: "   \n  ".to_string(),
            }),
        )
        .await
        .unwrap_err();

        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }
}
