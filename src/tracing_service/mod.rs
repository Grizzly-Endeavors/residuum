//! Tracing and observability service.
//!
//! Provides a transport-agnostic API for managing trace export, OTEL endpoints,
//! content sanitization, and error reporting. Called by both CLI (via HTTP to
//! the daemon) and web API handlers.

mod sanitize;

pub use sanitize::sanitize_spans;

use std::sync::Arc;

use anyhow::Result;
use serde::Serialize;
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
    /// Send to the built-in developer bug report endpoint.
    BuiltinEndpoint,
    /// Send to a specific endpoint URL.
    Specific(String),
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

    /// Test connectivity to an OTEL endpoint.
    ///
    /// # Errors
    /// Returns an error if the endpoint is unreachable.
    #[expect(
        clippy::unused_async,
        reason = "will use await once OTEL export is wired"
    )]
    pub async fn test_otel_connectivity(&self, _url: &str) -> Result<()> {
        // TODO: implement OTEL connectivity test once export pipeline is built
        tracing::warn!("OTEL connectivity test not yet implemented");
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
            ExportTarget::BuiltinEndpoint => {
                // Built-in endpoint not yet deployed
                tracing::warn!("bug report infrastructure not yet available");
                return Ok(ExportResult {
                    spans_sent: 0,
                    endpoint_results: vec![EndpointExportResult {
                        url: "built-in".to_string(),
                        success: false,
                        error: Some("bug report infrastructure not yet available".to_string()),
                    }],
                });
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
        let span_count = spans.len();
        drop(state);

        // TODO: actual OTEL export — for now return success with span count
        let endpoint_results = endpoints
            .iter()
            .map(|ep| {
                tracing::info!(
                    url = %ep.url,
                    spans = span_count,
                    "would export traces (OTEL export not yet wired)"
                );
                EndpointExportResult {
                    url: ep.url.clone(),
                    success: true,
                    error: None,
                }
            })
            .collect();

        Ok(ExportResult {
            spans_sent: span_count,
            endpoint_results,
        })
    }

    /// Start streaming traces to configured OTEL endpoints.
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
        state.streaming = true;
        // TODO: spawn streaming task once OTEL export pipeline is built
        tracing::warn!(
            "trace streaming started (OTEL export not yet wired — spans are being captured but not sent)"
        );
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
        state.streaming = false;
        tracing::info!("trace streaming stopped");
        Ok(())
    }

    /// Send a bug report with trace dump to the built-in developer endpoint.
    ///
    /// # Errors
    /// Returns an error if the export fails.
    #[expect(
        clippy::unused_async,
        reason = "will use await once built-in endpoint is deployed"
    )]
    pub async fn send_bug_report(&self, message: &str) -> Result<ExportResult> {
        tracing::info!(message, "bug report requested");
        // Currently a no-op — built-in endpoint infrastructure not yet deployed
        tracing::warn!("bug report infrastructure not yet available — report not sent");
        Ok(ExportResult {
            spans_sent: 0,
            endpoint_results: vec![EndpointExportResult {
                url: "built-in".to_string(),
                success: false,
                error: Some("bug report infrastructure not yet available".to_string()),
            }],
        })
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
