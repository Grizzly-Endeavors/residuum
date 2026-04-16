//! Tracing and observability service.
//!
//! Provides a transport-agnostic API for managing trace export, OTEL endpoints,
//! content sanitization, and error reporting. Called by both CLI (via HTTP to
//! the daemon) and web API handlers.

pub mod client_context;
mod otel;
mod sanitize;
mod submit;

pub use sanitize::sanitize_spans;

use std::sync::Arc;

use anyhow::Result;
use opentelemetry_sdk::trace::SpanExporter;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::config::{OtelEndpoint, TracingConfig};
use crate::util::telemetry::SpanBufferHandle;

/// Runtime tracing state that can diverge from persisted config.
///
/// Config changes (via reload) update this state. Runtime toggles (via API)
/// also update this state but are not persisted to config.toml.
struct TracingState {
    config: TracingConfig,
    streaming: bool,
    streaming_cancel: Option<tokio_util::sync::CancellationToken>,
}

/// Current status of the tracing subsystem (returned by status endpoints).
#[derive(Debug, Clone, Serialize)]
pub struct TracingStatus {
    /// Current log detail level.
    pub log_level: String,
    /// Whether automatic error reporting is enabled.
    pub auto_error_reporting: bool,
    /// Whether content sanitization is enabled.
    pub sanitize_content: bool,
    /// Configured OTEL endpoints.
    pub otel_endpoints: Vec<OtelEndpointStatus>,
    /// Whether trace streaming is active.
    pub streaming: bool,
    /// Number of spans currently in the buffer.
    pub buffer_size: usize,
}

/// OTEL endpoint info for status display.
#[derive(Debug, Clone, Serialize)]
pub struct OtelEndpointStatus {
    /// Endpoint URL.
    pub url: String,
    /// Human-readable name.
    pub name: Option<String>,
}

/// Result of a trace export operation.
#[derive(Debug, Clone, Serialize)]
pub struct ExportResult {
    /// Number of spans exported.
    pub spans_sent: usize,
    /// Per-endpoint results.
    pub endpoint_results: Vec<EndpointExportResult>,
}

/// Export result for a single endpoint.
#[derive(Debug, Clone, Serialize)]
pub struct EndpointExportResult {
    /// Endpoint URL.
    pub url: String,
    /// Whether the export succeeded.
    pub success: bool,
    /// Error message if export failed.
    pub error: Option<String>,
}

/// Where to send exported traces.
#[derive(Debug, Clone)]
pub enum ExportTarget {
    /// Send to all user-configured OTEL endpoints.
    UserEndpoints,
    /// Send to a specific endpoint URL.
    Specific(String),
}

/// User-selected severity for a bug report. Wire values are locked to
/// `broken | wrong | annoying` — anything else fails the ingest validator.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Something is fundamentally non-functional.
    Broken,
    /// Behavior is wrong but not blocking.
    Wrong,
    /// Behavior is annoying but functional.
    Annoying,
}

/// One entry in the active-subagent inventory attached to a bug report.
#[derive(Debug, Clone, Serialize)]
pub struct Subagent {
    /// Subagent identifier.
    pub name: String,
    /// Free-form runtime status string.
    pub status: String,
}

/// Runtime metadata gathered for a bug report submission.
#[derive(Debug, Clone, Serialize)]
pub struct ClientContext {
    /// Build version (`git describe --tags --always`).
    pub version: String,
    /// Short git commit SHA, when built from a git checkout.
    pub commit: Option<String>,
    /// `std::env::consts::OS` value.
    pub os: String,
    /// `std::env::consts::ARCH` value.
    pub arch: String,
    /// Active main provider kind (e.g. `"anthropic"`), if resolved.
    pub model_provider: Option<String>,
    /// Active main model name, if resolved.
    pub model_name: Option<String>,
    /// Live subagent inventory at submission time. Currently always empty.
    pub active_subagents: Vec<Subagent>,
    /// Curated allowlist of safe config flags. Currently always empty.
    pub config_flags: std::collections::BTreeMap<String, String>,
}

/// Reduced client context used for the feedback endpoint. The feedback
/// wire contract accepts `version` only — using a distinct type keeps
/// the over-attachment risk at zero.
#[derive(Debug, Clone, Serialize)]
pub struct FeedbackClient {
    /// Build version.
    pub version: String,
}

/// A complete bug report ready to submit.
#[derive(Debug, Clone)]
pub struct BugReport {
    /// What actually happened.
    pub what_happened: String,
    /// What the user expected to happen.
    pub what_expected: String,
    /// What the user was doing when the issue occurred.
    pub what_doing: String,
    /// Severity selected by the user.
    pub severity: Severity,
    /// Runtime metadata gathered by [`client_context::gather_for_bug_report`].
    pub client: ClientContext,
}

/// A feedback submission ready to send.
#[derive(Debug, Clone)]
pub struct Feedback {
    /// Free-form feedback message.
    pub message: String,
    /// Optional user-supplied category tag.
    pub category: Option<String>,
    /// Version-only client metadata.
    pub client: FeedbackClient,
}

/// Receipt returned by the upstream ingest service on successful
/// submission. Contains the public reference ID and submission time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmissionReceipt {
    /// Public reference ID (e.g. `"RR-01HZKX2P7M"`).
    pub public_id: String,
    /// ISO-8601 submission timestamp.
    pub submitted_at: String,
}

/// The tracing service — owns runtime state and the span buffer handle.
pub struct TracingService {
    state: Arc<RwLock<TracingState>>,
    span_buffer: SpanBufferHandle,
}

impl TracingService {
    /// Create a new tracing service from config and span buffer.
    #[must_use]
    pub fn new(config: TracingConfig, span_buffer: SpanBufferHandle) -> Self {
        Self {
            state: Arc::new(RwLock::new(TracingState {
                config,
                streaming: false,
                streaming_cancel: None,
            })),
            span_buffer,
        }
    }

    /// Get the current tracing status.
    pub async fn status(&self) -> TracingStatus {
        let state = self.state.read().await;
        TracingStatus {
            log_level: state.config.log_level.to_string(),
            auto_error_reporting: state.config.auto_error_reporting,
            sanitize_content: state.config.sanitize_content,
            otel_endpoints: state
                .config
                .otel_endpoints
                .iter()
                .map(|ep| OtelEndpointStatus {
                    url: ep.url.clone(),
                    name: ep.name.clone(),
                })
                .collect(),
            streaming: state.streaming,
            buffer_size: self.span_buffer.len(),
        }
    }

    /// Update the tracing config (called on config reload).
    pub async fn update_config(&self, config: TracingConfig) {
        let mut state = self.state.write().await;
        state.config = config;
    }

    /// Set automatic error reporting on or off.
    pub async fn set_auto_error_reporting(&self, enabled: bool) {
        let mut state = self.state.write().await;
        state.config.auto_error_reporting = enabled;
    }

    /// Set content sanitization on or off.
    pub async fn set_sanitize_content(&self, enabled: bool) {
        let mut state = self.state.write().await;
        state.config.sanitize_content = enabled;
    }

    /// Add an OTEL endpoint.
    ///
    /// # Errors
    /// Returns an error if an endpoint with the same URL already exists.
    pub async fn add_otel_endpoint(&self, endpoint: OtelEndpoint) -> Result<()> {
        let mut state = self.state.write().await;
        if state
            .config
            .otel_endpoints
            .iter()
            .any(|ep| ep.url == endpoint.url)
        {
            anyhow::bail!("endpoint already configured: {}", endpoint.url);
        }
        state.config.otel_endpoints.push(endpoint);
        Ok(())
    }

    /// Remove an OTEL endpoint by URL.
    ///
    /// # Errors
    /// Returns an error if no endpoint with the given URL exists.
    pub async fn remove_otel_endpoint(&self, url: &str) -> Result<()> {
        let mut state = self.state.write().await;
        let before = state.config.otel_endpoints.len();
        state.config.otel_endpoints.retain(|ep| ep.url != url);
        if state.config.otel_endpoints.len() == before {
            anyhow::bail!("no endpoint configured with url: {url}");
        }
        Ok(())
    }

    /// List configured OTEL endpoints.
    pub async fn list_otel_endpoints(&self) -> Vec<OtelEndpoint> {
        let state = self.state.read().await;
        state.config.otel_endpoints.clone()
    }

    /// Test connectivity to an OTEL endpoint by sending an empty export.
    ///
    /// # Errors
    /// Returns an error if the endpoint is unreachable or the exporter fails to build.
    pub async fn test_otel_connectivity(&self, url: &str) -> Result<()> {
        let endpoint = OtelEndpoint {
            url: url.to_string(),
            name: None,
            headers: std::collections::HashMap::new(),
        };
        let exporter = otel::build_exporter(&endpoint)
            .map_err(|e| anyhow::anyhow!("failed to build exporter for {url}: {e}"))?;
        // Send an empty batch to test connectivity
        exporter
            .export(Vec::new())
            .await
            .map_err(|e| anyhow::anyhow!("connectivity test failed for {url}: {e}"))?;
        tracing::info!(url, "OTEL connectivity test passed");
        Ok(())
    }

    /// Export a snapshot of buffered traces to the target endpoints.
    ///
    /// # Errors
    /// Returns an error if no endpoints are configured for the target, or if export fails.
    pub async fn dump_traces(&self, target: ExportTarget) -> Result<ExportResult> {
        let state = self.state.read().await;

        let endpoints = match &target {
            ExportTarget::UserEndpoints => {
                if state.config.otel_endpoints.is_empty() {
                    anyhow::bail!(
                        "no OTEL endpoints configured; add one with 'residuum tracing otel add <url>'"
                    );
                }
                state.config.otel_endpoints.clone()
            }
            ExportTarget::Specific(url) => {
                vec![OtelEndpoint {
                    url: url.clone(),
                    name: None,
                    headers: std::collections::HashMap::new(),
                }]
            }
        };

        let mut spans = self.span_buffer.snapshot();
        if state.config.sanitize_content {
            sanitize_spans(&mut spans);
        }
        drop(state);

        let otel_spans = otel::convert_spans(&spans);
        let span_count = otel_spans.len();

        let mut endpoint_results = Vec::with_capacity(endpoints.len());
        for ep in &endpoints {
            let result = export_to_endpoint(ep, otel_spans.clone()).await;
            endpoint_results.push(result);
        }

        Ok(ExportResult {
            spans_sent: span_count,
            endpoint_results,
        })
    }

    /// Start streaming traces to configured OTEL endpoints.
    ///
    /// Spawns a background task that periodically drains the span buffer and
    /// exports to all configured endpoints.
    ///
    /// # Errors
    /// Returns an error if streaming is already active or no endpoints are configured.
    pub async fn start_streaming(&self) -> Result<()> {
        let mut state = self.state.write().await;
        if state.streaming {
            anyhow::bail!("trace streaming is already active");
        }
        if state.config.otel_endpoints.is_empty() {
            anyhow::bail!(
                "no OTEL endpoints configured; add one with 'residuum tracing otel add <url>'"
            );
        }

        let cancel = tokio_util::sync::CancellationToken::new();
        let cancel_clone = cancel.clone();
        let buffer = self.span_buffer.clone();
        let service_state = Arc::clone(&self.state);

        tokio::spawn(async move {
            tracing::info!("trace streaming task started");
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
            loop {
                tokio::select! {
                    () = cancel_clone.cancelled() => {
                        tracing::info!("trace streaming task cancelled");
                        break;
                    }
                    _ = interval.tick() => {
                        let drained = buffer.drain();
                        if drained.is_empty() {
                            continue;
                        }

                        let cfg_snapshot = service_state.read().await;
                        let mut spans = drained;
                        if cfg_snapshot.config.sanitize_content {
                            sanitize_spans(&mut spans);
                        }
                        let endpoints = cfg_snapshot.config.otel_endpoints.clone();
                        drop(cfg_snapshot);

                        let otel_spans = otel::convert_spans(&spans);
                        for ep in &endpoints {
                            let result = export_to_endpoint(ep, otel_spans.clone()).await;
                            if !result.success {
                                tracing::warn!(
                                    url = %result.url,
                                    error = ?result.error,
                                    "streaming export failed"
                                );
                            }
                        }
                    }
                }
            }
        });

        state.streaming = true;
        state.streaming_cancel = Some(cancel);
        tracing::info!("trace streaming started");
        Ok(())
    }

    /// Stop streaming traces.
    ///
    /// # Errors
    /// Returns an error if streaming is not active.
    pub async fn stop_streaming(&self) -> Result<()> {
        let mut state = self.state.write().await;
        if !state.streaming {
            anyhow::bail!("trace streaming is not active");
        }
        if let Some(cancel) = state.streaming_cancel.take() {
            cancel.cancel();
        }
        state.streaming = false;
        tracing::info!("trace streaming stopped");
        Ok(())
    }

    /// Submit a bug report (with sanitized OTLP trace dump) to the
    /// upstream relay.
    ///
    /// # Errors
    /// Returns an error if the upstream is unreachable, returns a
    /// non-2xx response, or returns a body that doesn't match the
    /// documented receipt shape.
    pub async fn send_bug_report(&self, report: BugReport) -> Result<SubmissionReceipt> {
        let endpoint = self.state.read().await.config.feedback_endpoint.clone();
        let spans = self.span_buffer.snapshot();
        submit::submit_bug_report(&endpoint, report, spans).await
    }

    /// Submit lightweight feedback (no trace dump) to the upstream relay.
    ///
    /// # Errors
    /// Returns an error if the upstream is unreachable or returns a
    /// non-2xx response.
    pub async fn send_feedback(&self, feedback: Feedback) -> Result<SubmissionReceipt> {
        let endpoint = self.state.read().await.config.feedback_endpoint.clone();
        submit::submit_feedback(&endpoint, feedback).await
    }

    /// Called from error paths to auto-report if enabled.
    ///
    /// This is a fire-and-forget operation — errors are logged, not propagated.
    pub async fn on_error(&self, error_context: &str) {
        let state = self.state.read().await;
        if !state.config.auto_error_reporting {
            return;
        }
        drop(state);
        // Currently a no-op — built-in endpoint infrastructure not yet deployed
        tracing::debug!(
            context = error_context,
            "auto error reporting triggered (infrastructure not yet available)"
        );
    }
}

/// Export OTEL spans to a single endpoint. Returns per-endpoint result.
async fn export_to_endpoint(
    endpoint: &OtelEndpoint,
    spans: Vec<opentelemetry_sdk::trace::SpanData>,
) -> EndpointExportResult {
    let exporter = otel::build_exporter(endpoint);
    match exporter {
        Ok(exp) => match exp.export(spans).await {
            Ok(()) => {
                tracing::debug!(url = %endpoint.url, "trace export succeeded");
                EndpointExportResult {
                    url: endpoint.url.clone(),
                    success: true,
                    error: None,
                }
            }
            Err(e) => {
                tracing::warn!(url = %endpoint.url, error = %e, "trace export failed");
                EndpointExportResult {
                    url: endpoint.url.clone(),
                    success: false,
                    error: Some(format!("{e}")),
                }
            }
        },
        Err(e) => {
            tracing::warn!(url = %endpoint.url, error = %e, "failed to build exporter");
            EndpointExportResult {
                url: endpoint.url.clone(),
                success: false,
                error: Some(format!("failed to build exporter: {e}")),
            }
        }
    }
}
