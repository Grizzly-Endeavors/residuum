//! Tracing and observability API endpoints.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::Deserialize;

use crate::config::OtelEndpoint;
use crate::tracing_service::{
    BugReport, ClientContext, ExportResult, ExportTarget, Feedback, Severity, SubmissionReceipt,
    TracingService, TracingStatus, client_context,
};

/// Shared state for the tracing API routes.
#[derive(Clone)]
pub(crate) struct TracingApiState {
    pub service: Arc<TracingService>,
    /// Snapshot of the runtime client context, gathered at gateway
    /// startup (or reload). Bug-report handlers attach this to every
    /// submission so the user-facing forms only carry user-typed fields.
    pub client_context: Arc<ClientContext>,
}

/// `GET /api/tracing/status`
pub(crate) async fn api_tracing_status(
    State(state): State<TracingApiState>,
) -> Json<TracingStatus> {
    Json(state.service.status().await)
}

/// Request body for toggling a boolean setting.
#[derive(Deserialize)]
pub(crate) struct ToggleRequest {
    enabled: bool,
}

/// `POST /api/tracing/error-reporting`
pub(crate) async fn api_tracing_error_reporting(
    State(state): State<TracingApiState>,
    Json(body): Json<ToggleRequest>,
) -> StatusCode {
    state.service.set_auto_error_reporting(body.enabled).await;
    StatusCode::OK
}

/// `POST /api/tracing/sanitize`
pub(crate) async fn api_tracing_sanitize(
    State(state): State<TracingApiState>,
    Json(body): Json<ToggleRequest>,
) -> StatusCode {
    state.service.set_sanitize_content(body.enabled).await;
    StatusCode::OK
}

/// Request body for adding an OTEL endpoint.
#[derive(Deserialize)]
pub(crate) struct AddEndpointRequest {
    url: String,
    name: Option<String>,
    headers: Option<HashMap<String, String>>,
}

/// `GET /api/tracing/otel/endpoints`
pub(crate) async fn api_tracing_otel_list(
    State(state): State<TracingApiState>,
) -> Json<Vec<crate::tracing_service::OtelEndpointStatus>> {
    let endpoints = state.service.list_otel_endpoints().await;
    Json(
        endpoints
            .into_iter()
            .map(|ep| crate::tracing_service::OtelEndpointStatus {
                url: ep.url,
                name: ep.name,
            })
            .collect(),
    )
}

/// `POST /api/tracing/otel/endpoints`
pub(crate) async fn api_tracing_otel_add(
    State(state): State<TracingApiState>,
    Json(body): Json<AddEndpointRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let endpoint = OtelEndpoint {
        url: body.url,
        name: body.name,
        headers: body.headers.unwrap_or_default(),
    };
    state
        .service
        .add_otel_endpoint(endpoint)
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("{e}")))?;
    Ok(StatusCode::CREATED)
}

/// Request body for removing an OTEL endpoint.
#[derive(Deserialize)]
pub(crate) struct RemoveEndpointRequest {
    url: String,
}

/// `DELETE /api/tracing/otel/endpoints`
pub(crate) async fn api_tracing_otel_remove(
    State(state): State<TracingApiState>,
    Json(body): Json<RemoveEndpointRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    state
        .service
        .remove_otel_endpoint(&body.url)
        .await
        .map_err(|e| (StatusCode::NOT_FOUND, format!("{e}")))?;
    Ok(StatusCode::OK)
}

/// Request body for testing OTEL connectivity.
#[derive(Deserialize)]
pub(crate) struct TestConnectivityRequest {
    url: String,
}

/// `POST /api/tracing/otel/test`
pub(crate) async fn api_tracing_otel_test(
    State(state): State<TracingApiState>,
    Json(body): Json<TestConnectivityRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    state
        .service
        .test_otel_connectivity(&body.url)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("{e}")))?;
    Ok(StatusCode::OK)
}

/// `POST /api/tracing/dump`
pub(crate) async fn api_tracing_dump(
    State(state): State<TracingApiState>,
) -> Result<Json<ExportResult>, (StatusCode, String)> {
    state
        .service
        .dump_traces(ExportTarget::UserEndpoints)
        .await
        .map(Json)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("{e}")))
}

/// `POST /api/tracing/stream/start`
pub(crate) async fn api_tracing_stream_start(
    State(state): State<TracingApiState>,
) -> Result<StatusCode, (StatusCode, String)> {
    state
        .service
        .start_streaming()
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("{e}")))?;
    Ok(StatusCode::OK)
}

/// `POST /api/tracing/stream/stop`
pub(crate) async fn api_tracing_stream_stop(
    State(state): State<TracingApiState>,
) -> Result<StatusCode, (StatusCode, String)> {
    state
        .service
        .stop_streaming()
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("{e}")))?;
    Ok(StatusCode::OK)
}

/// Request body for the bug-report endpoint. Mirrors the user-supplied
/// fields of the upstream wire contract; runtime metadata (`client`)
/// is attached server-side from the snapshot on `TracingApiState`.
#[derive(Deserialize)]
pub(crate) struct BugReportRequest {
    what_happened: String,
    what_expected: String,
    what_doing: String,
    severity: Severity,
}

/// `POST /api/tracing/bug-report`
pub(crate) async fn api_tracing_bug_report(
    State(state): State<TracingApiState>,
    Json(body): Json<BugReportRequest>,
) -> Result<Json<SubmissionReceipt>, (StatusCode, String)> {
    let report = BugReport {
        what_happened: body.what_happened,
        what_expected: body.what_expected,
        what_doing: body.what_doing,
        severity: body.severity,
        client: (*state.client_context).clone(),
    };
    state
        .service
        .send_bug_report(report)
        .await
        .map(Json)
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("{e}")))
}

/// Request body for the feedback endpoint.
#[derive(Deserialize)]
pub(crate) struct FeedbackRequest {
    message: String,
    #[serde(default)]
    category: Option<String>,
}

/// `POST /api/tracing/feedback`
pub(crate) async fn api_tracing_feedback(
    State(state): State<TracingApiState>,
    Json(body): Json<FeedbackRequest>,
) -> Result<Json<SubmissionReceipt>, (StatusCode, String)> {
    let feedback = Feedback {
        message: body.message,
        category: body.category,
        client: client_context::gather_for_feedback(),
    };
    state
        .service
        .send_feedback(feedback)
        .await
        .map(Json)
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("{e}")))
}
