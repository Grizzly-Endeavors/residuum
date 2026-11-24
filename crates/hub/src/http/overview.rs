use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::response::{IntoResponse, Json, Response};
use axum::routing::get;

use crate::overview::TeamOverview;

pub(super) fn routes(overview: Arc<TeamOverview>) -> Router {
    Router::new()
        .route("/api/hub/overview", get(get_overview))
        .with_state(overview)
}

async fn get_overview(State(overview): State<Arc<TeamOverview>>) -> Response {
    Json(overview.snapshot().await).into_response()
}
