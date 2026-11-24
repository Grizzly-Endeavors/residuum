use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::Deserialize;

use residuum_config::OtelEndpoint;
use residuum_core::build_info::BuildInfo;
use residuum_tracing_service::client_context;
use residuum_tracing_service::{
    BugReport, ClientContext, ExportResult, ExportTarget, Feedback, Severity, SubmissionReceipt,
    TracingService, TracingStatus,
};

#[derive(Clone)]
pub(crate) struct TracingApiState {
    pub service: Arc<TracingService>,
    pub build: BuildInfo,
    pub client_context: Arc<ClientContext>,
    pub active_subagents: Arc<dyn Fn() -> Vec<residuum_tracing_service::Subagent> + Send + Sync>,
}

pub(crate) async fn api_tracing_status(
    State(state): State<TracingApiState>,
) -> Json<TracingStatus> {
    Json(state.service.status().await)
}

#[derive(Deserialize)]
pub(crate) struct ToggleRequest {
    enabled: bool,
}

pub(crate) async fn api_tracing_error_reporting(
    State(state): State<TracingApiState>,
    Json(body): Json<ToggleRequest>,
) -> StatusCode {
    state.service.set_auto_error_reporting(body.enabled).await;
    StatusCode::OK
}

pub(crate) async fn api_tracing_sanitize(
    State(state): State<TracingApiState>,
    Json(body): Json<ToggleRequest>,
) -> StatusCode {
    state.service.set_sanitize_content(body.enabled).await;
    StatusCode::OK
}

#[derive(Deserialize)]
pub(crate) struct AddEndpointRequest {
    url: String,
    name: Option<String>,
    headers: Option<HashMap<String, String>>,
}

pub(crate) async fn api_tracing_otel_list(
    State(state): State<TracingApiState>,
) -> Json<Vec<residuum_tracing_service::OtelEndpointStatus>> {
    let endpoints = state.service.list_otel_endpoints().await;
    Json(
        endpoints
            .into_iter()
            .map(|ep| residuum_tracing_service::OtelEndpointStatus {
                url: ep.url,
                name: ep.name,
            })
            .collect(),
    )
}

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

#[derive(Deserialize)]
pub(crate) struct RemoveEndpointRequest {
    url: String,
}

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

#[derive(Deserialize)]
pub(crate) struct TestConnectivityRequest {
    url: String,
}

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

#[derive(Deserialize)]
pub(crate) struct BugReportRequest {
    what_happened: String,
    what_expected: String,
    what_doing: String,
    severity: Severity,
}

pub(crate) async fn api_tracing_bug_report(
    State(state): State<TracingApiState>,
    Json(body): Json<BugReportRequest>,
) -> Result<Json<SubmissionReceipt>, (StatusCode, String)> {
    let mut client = (*state.client_context).clone();
    client.active_subagents = (state.active_subagents)();
    let report = BugReport {
        what_happened: body.what_happened,
        what_expected: body.what_expected,
        what_doing: body.what_doing,
        severity: body.severity,
        client,
    };
    state
        .service
        .send_bug_report(report)
        .await
        .map(Json)
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("{e}")))
}

#[derive(Deserialize)]
pub(crate) struct FeedbackRequest {
    message: String,
    #[serde(default)]
    category: Option<String>,
}

pub(crate) async fn api_tracing_feedback(
    State(state): State<TracingApiState>,
    Json(body): Json<FeedbackRequest>,
) -> Result<Json<SubmissionReceipt>, (StatusCode, String)> {
    let feedback = Feedback {
        message: body.message,
        category: body.category,
        client: client_context::gather_for_feedback(state.build),
    };
    state
        .service
        .send_feedback(feedback)
        .await
        .map(Json)
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("{e}")))
}
