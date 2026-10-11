//! `GET /api/hub/events`: the team event log, newest entry first.
//!
//! Failures are `{ "error": message }` with `400` for a query that can't be
//! read.

use std::sync::Arc;

use axum::Router;
use axum::extract::Query;
use axum::extract::State;
use axum::extract::rejection::QueryRejection;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use axum::routing::get;
use serde::Deserialize;

use super::error::json_error;
use crate::hub::team_events::{PageQuery, TeamEventLog};

/// The events route.
pub(super) fn routes(log: Arc<TeamEventLog>) -> Router {
    Router::new()
        .route("/api/hub/events", get(list_events))
        .with_state(log)
}

/// The query of `GET /api/hub/events`, kept as text so a value that can't be
/// used is answered with a JSON error that says which one.
#[derive(Deserialize)]
struct ListParams {
    before: Option<String>,
    after: Option<String>,
    limit: Option<String>,
}

impl ListParams {
    fn into_query(self) -> Result<PageQuery, String> {
        Ok(PageQuery {
            before: self
                .before
                .as_deref()
                .map(|raw| id_param("before", raw))
                .transpose()?,
            after: self
                .after
                .as_deref()
                .map(|raw| id_param("after", raw))
                .transpose()?,
            limit: self.limit.as_deref().map(limit_param).transpose()?,
        })
    }
}

fn id_param(name: &str, raw: &str) -> Result<u64, String> {
    raw.parse::<u64>()
        .map_err(|e| format!("the {name} id must be a whole number, not '{raw}' ({e})"))
}

fn limit_param(raw: &str) -> Result<usize, String> {
    let limit = raw
        .parse::<usize>()
        .map_err(|e| format!("the limit must be a whole number, not '{raw}' ({e})"))?;
    if limit == 0 {
        return Err("the limit must be at least 1".to_string());
    }
    Ok(limit)
}

/// `GET /api/hub/events?before=&after=&limit=`.
async fn list_events(
    State(log): State<Arc<TeamEventLog>>,
    params: Result<Query<ListParams>, QueryRejection>,
) -> Response {
    let query = match params {
        Ok(Query(params)) => match params.into_query() {
            Ok(query) => query,
            Err(message) => return json_error(StatusCode::BAD_REQUEST, message),
        },
        Err(rejection) => return json_error(StatusCode::BAD_REQUEST, rejection.body_text()),
    };
    Json(log.page(&query)).into_response()
}
