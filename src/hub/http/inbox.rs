//! `/api/hub/inbox...`: every agent's user inbox in one place.
//!
//! The routes read and change the agents' inbox files through
//! [`crate::hub::inbox`], so an agent answers in any state. Failures are
//! `{ "error": message }`: `400` for a request that can't be read, `404` for
//! an unknown agent or item, `409` when a move would replace another item, and
//! `500` when the files can't be read or changed. An item the hub changed
//! through these routes is counted again in its agent's overview.

use std::sync::Arc;

use axum::Router;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post, put};
use serde::Deserialize;

use super::error::json_error;
use crate::hub::AgentDirectory;
use crate::hub::inbox::{self, HubInboxError, HubInboxItem, InboxStatus, ListQuery};
use crate::hub::overview::TeamOverview;

/// What the inbox routes read and tell.
#[derive(Clone)]
pub(super) struct InboxState {
    /// Where the agents' files are.
    pub directory: Arc<dyn AgentDirectory>,
    /// Told when the hub changes an item, so the agent's unread count is
    /// read again.
    pub overview: Arc<TeamOverview>,
}

/// The list, unread count, and per-item routes.
pub(super) fn routes(state: InboxState) -> Router {
    Router::new()
        .route("/api/hub/inbox", get(list_inbox))
        .route("/api/hub/inbox/unread", get(unread_inbox))
        .route("/api/hub/inbox/{agent}/{id}/read", put(read_item))
        .route("/api/hub/inbox/{agent}/{id}/archive", post(archive_item))
        .route("/api/hub/inbox/{agent}/{id}/restore", post(restore_item))
        .with_state(state)
}

/// The query of `GET /api/hub/inbox`, kept as text so a value that can't be
/// used is answered with a JSON error that says which one.
#[derive(Deserialize)]
struct ListParams {
    status: Option<String>,
    agent: Option<String>,
    before: Option<String>,
    limit: Option<String>,
}

impl ListParams {
    fn into_query(self) -> Result<ListQuery, String> {
        let status = match self.status.as_deref() {
            None | Some("active") => InboxStatus::Active,
            Some("archived") => InboxStatus::Archived,
            Some(other) => {
                return Err(format!(
                    "the status must be 'active' or 'archived', not '{other}'"
                ));
            }
        };
        let limit = self
            .limit
            .as_deref()
            .map(|raw| {
                raw.parse::<usize>()
                    .map_err(|e| format!("the limit must be a whole number, not '{raw}' ({e})"))
            })
            .transpose()?;
        Ok(ListQuery {
            status,
            agent: self.agent,
            before: self.before,
            limit,
        })
    }
}

fn inbox_error_response(error: &HubInboxError) -> Response {
    let status = match error {
        HubInboxError::UnknownAgent(_) | HubInboxError::UnknownItem(_) => StatusCode::NOT_FOUND,
        HubInboxError::BadRequest(_) => StatusCode::BAD_REQUEST,
        HubInboxError::Conflict(_) => StatusCode::CONFLICT,
        HubInboxError::Failed(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    json_error(status, error.to_string())
}

/// `GET /api/hub/inbox` — one page of items, newest first.
async fn list_inbox(
    State(state): State<InboxState>,
    params: Result<Query<ListParams>, QueryRejection>,
) -> Response {
    let query = match params {
        Ok(Query(params)) => match params.into_query() {
            Ok(query) => query,
            Err(message) => return json_error(StatusCode::BAD_REQUEST, message),
        },
        Err(rejection) => return json_error(StatusCode::BAD_REQUEST, rejection.body_text()),
    };
    match inbox::list(state.directory.as_ref(), &query).await {
        Ok(page) => Json(page).into_response(),
        Err(e) => inbox_error_response(&e),
    }
}

/// `GET /api/hub/inbox/unread` — unread items, in total and per agent.
async fn unread_inbox(State(state): State<InboxState>) -> Response {
    match inbox::unread(state.directory.as_ref()).await {
        Ok(unread) => Json(unread).into_response(),
        Err(e) => inbox_error_response(&e),
    }
}

/// The `{ "item": ... }` body of the per-item routes. A changed item is told
/// to the overview, which counts the agent's inbox again.
async fn item_response(
    overview: &TeamOverview,
    agent: &str,
    result: Result<HubInboxItem, HubInboxError>,
) -> Response {
    match result {
        Ok(item) => {
            overview.inbox_changed(agent).await;
            Json(serde_json::json!({ "item": item })).into_response()
        }
        Err(e) => inbox_error_response(&e),
    }
}

/// `PUT /api/hub/inbox/{agent}/{id}/read`.
async fn read_item(
    State(state): State<InboxState>,
    Path((agent, id)): Path<(String, String)>,
) -> Response {
    let result = inbox::mark_read(state.directory.as_ref(), &agent, &id).await;
    item_response(&state.overview, &agent, result).await
}

/// `POST /api/hub/inbox/{agent}/{id}/archive`.
async fn archive_item(
    State(state): State<InboxState>,
    Path((agent, id)): Path<(String, String)>,
) -> Response {
    let result = inbox::archive(state.directory.as_ref(), &agent, &id).await;
    item_response(&state.overview, &agent, result).await
}

/// `POST /api/hub/inbox/{agent}/{id}/restore`.
async fn restore_item(
    State(state): State<InboxState>,
    Path((agent, id)): Path<(String, String)>,
) -> Response {
    let result = inbox::restore(state.directory.as_ref(), &agent, &id).await;
    item_response(&state.overview, &agent, result).await
}
