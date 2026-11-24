pub mod client_context;
#[cfg(test)]
#[path = "../../core/src/testing/crypto.rs"]
mod crypto;
mod otel;
mod sanitize;
mod submit;

pub(crate) use sanitize::{redact_agent_key_values, sanitize_spans};

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use opentelemetry_sdk::trace::SpanExporter;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, RwLock};

use residuum_config::{OtelEndpoint, TracingConfig};
use residuum_core::util::telemetry::{CompletedSpan, SpanBufferHandle};

struct TracingState {
    config: TracingConfig,
    streaming: bool,
    streaming_cancel: Option<tokio_util::sync::CancellationToken>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TracingStatus {
    pub log_level: String,
    pub auto_error_reporting: bool,
    pub sanitize_content: bool,
    pub otel_endpoints: Vec<OtelEndpointStatus>,
    pub streaming: bool,
    pub buffer_size: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct OtelEndpointStatus {
    pub url: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExportResult {
    pub spans_sent: usize,
    pub endpoint_results: Vec<EndpointExportResult>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EndpointExportResult {
    pub url: String,
    pub success: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum ExportTarget {
    UserEndpoints,
    Specific(String),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Broken,
    Wrong,
    Annoying,
}

#[derive(Debug, Clone, Serialize)]
pub struct Subagent {
    pub name: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClientContext {
    pub version: String,
    pub commit: Option<String>,
    pub os: String,
    pub arch: String,
    pub model_provider: Option<String>,
    pub model_name: Option<String>,
    pub active_subagents: Vec<Subagent>,
    pub config_flags: std::collections::BTreeMap<String, String>,
    #[serde(skip)]
    pub agent: Option<String>,
}

/// Reduced client context used for the feedback endpoint. The feedback
/// wire contract accepts `version` only — using a distinct type keeps
/// the over-attachment risk at zero.
#[derive(Debug, Clone, Serialize)]
pub struct FeedbackClient {
    pub version: String,
}

#[derive(Debug, Clone)]
pub struct BugReport {
    pub what_happened: String,
    pub what_expected: String,
    pub what_doing: String,
    pub severity: Severity,
    pub client: ClientContext,
}

#[derive(Debug, Clone)]
pub struct Feedback {
    pub message: String,
    pub category: Option<String>,
    pub client: FeedbackClient,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmissionReceipt {
    pub public_id: String,
    pub submitted_at: String,
}

/// In-memory rate limiter for [`TracingService::on_error`], so a tight
/// error loop can't burn through the relay's 5-per-hour-per-IP submission
/// budget. Admits at most one report per minute for a given error context,
/// and at most five reports total in any rolling hour.
struct ErrorReportLimiter {
    last_admitted: HashMap<u64, Instant>,
    hourly: VecDeque<Instant>,
}

impl ErrorReportLimiter {
    const PER_KEY_INTERVAL: Duration = Duration::from_secs(60);
    const HOURLY_CAP: usize = 5;
    const HOURLY_WINDOW: Duration = Duration::from_secs(3600);

    fn new() -> Self {
        Self {
            last_admitted: HashMap::new(),
            hourly: VecDeque::new(),
        }
    }

    fn admit(&mut self, error_context: &str, now: Instant) -> bool {
        while let Some(&oldest) = self.hourly.front() {
            if now.duration_since(oldest) > Self::HOURLY_WINDOW {
                self.hourly.pop_front();
            } else {
                break;
            }
        }
        if self.hourly.len() >= Self::HOURLY_CAP {
            return false;
        }

        self.last_admitted
            .retain(|_, last| now.duration_since(*last) < Self::PER_KEY_INTERVAL);

        let mut hasher = DefaultHasher::new();
        error_context.hash(&mut hasher);
        let key = hasher.finish();

        if self.last_admitted.contains_key(&key) {
            return false;
        }

        self.last_admitted.insert(key, now);
        self.hourly.push_back(now);
        true
    }
}

pub struct TracingService {
    state: Arc<RwLock<TracingState>>,
    span_buffer: SpanBufferHandle,
    agent_keys: Option<residuum_config::agent_keys::SharedAgentKeys>,
    error_report_limiter: Mutex<ErrorReportLimiter>,
}

async fn redact_with(
    agent_keys: Option<&residuum_config::agent_keys::SharedAgentKeys>,
    spans: &mut [CompletedSpan],
) {
    if let Some(keys) = agent_keys {
        redact_agent_key_values(spans, &keys.redactor().await);
    }
}

impl TracingService {
    #[must_use]
    pub fn new(config: TracingConfig, span_buffer: SpanBufferHandle) -> Self {
        Self {
            state: Arc::new(RwLock::new(TracingState {
                config,
                streaming: false,
                streaming_cancel: None,
            })),
            span_buffer,
            agent_keys: None,
            error_report_limiter: Mutex::new(ErrorReportLimiter::new()),
        }
    }

    #[must_use]
    pub fn with_agent_keys(
        mut self,
        agent_keys: residuum_config::agent_keys::SharedAgentKeys,
    ) -> Self {
        self.agent_keys = Some(agent_keys);
        self
    }

    async fn redact_agent_keys(&self, spans: &mut [CompletedSpan]) {
        redact_with(self.agent_keys.as_ref(), spans).await;
    }

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

    pub async fn update_config(&self, config: TracingConfig) {
        let mut state = self.state.write().await;
        state.config = config;
    }

    pub async fn set_auto_error_reporting(&self, enabled: bool) {
        let mut state = self.state.write().await;
        state.config.auto_error_reporting = enabled;
    }

    pub async fn set_sanitize_content(&self, enabled: bool) {
        let mut state = self.state.write().await;
        state.config.sanitize_content = enabled;
    }

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

    pub async fn remove_otel_endpoint(&self, url: &str) -> Result<()> {
        let mut state = self.state.write().await;
        let before = state.config.otel_endpoints.len();
        state.config.otel_endpoints.retain(|ep| ep.url != url);
        if state.config.otel_endpoints.len() == before {
            anyhow::bail!("no endpoint configured with url: {url}");
        }
        Ok(())
    }

    pub async fn list_otel_endpoints(&self) -> Vec<OtelEndpoint> {
        let state = self.state.read().await;
        state.config.otel_endpoints.clone()
    }

    pub async fn test_otel_connectivity(&self, url: &str) -> Result<()> {
        let endpoint = OtelEndpoint {
            url: url.to_string(),
            name: None,
            headers: std::collections::HashMap::new(),
        };
        let otlp = otel::build_exporter(&endpoint)
            .map_err(|e| anyhow::anyhow!("failed to build exporter for {url}: {e}"))?;
        if let Err(e) = otlp.exporter.export(Vec::new()).await {
            let error = otlp.transport.describe_failure(&e);
            tracing::warn!(
                url,
                attempts = otlp.transport.attempts(),
                error = %error,
                "OTEL connectivity test failed"
            );
            anyhow::bail!("connectivity test failed for {url}: {error}");
        }
        tracing::info!(url, "OTEL connectivity test passed");
        Ok(())
    }

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
        self.redact_agent_keys(&mut spans).await;

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
        let agent_keys = self.agent_keys.clone();

        residuum_core::util::spawn_in_span(async move {
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
                        redact_with(agent_keys.as_ref(), &mut spans).await;

                        let otel_spans = otel::convert_spans(&spans);
                        for ep in &endpoints {
                            export_to_endpoint(ep, otel_spans.clone()).await;
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
        let mut spans = self.span_buffer.snapshot();
        self.redact_agent_keys(&mut spans).await;
        submit::submit_bug_report(&endpoint, report, spans).await
    }

    pub async fn send_feedback(&self, feedback: Feedback) -> Result<SubmissionReceipt> {
        let endpoint = self.state.read().await.config.feedback_endpoint.clone();
        submit::submit_feedback(&endpoint, feedback).await
    }

    pub async fn on_error(&self, error_context: &str, client: ClientContext) {
        let state = self.state.read().await;
        if !state.config.auto_error_reporting {
            return;
        }
        drop(state);

        let limiter_key = match &client.agent {
            Some(agent) => format!("{agent}: {error_context}"),
            None => error_context.to_string(),
        };
        let admitted = self
            .error_report_limiter
            .lock()
            .await
            .admit(&limiter_key, Instant::now());
        if !admitted {
            tracing::debug!(
                context = error_context,
                "auto error report suppressed by rate limit"
            );
            return;
        }

        let (what_happened, what_doing) = match &client.agent {
            Some(agent) => (
                format!("agent '{agent}': {error_context}"),
                format!("auto-reported (no user input) while running agent '{agent}'"),
            ),
            None => (
                error_context.to_string(),
                "auto-reported (no user input)".to_string(),
            ),
        };
        let report = BugReport {
            what_happened,
            what_expected: "no error".to_string(),
            what_doing,
            severity: Severity::Broken,
            client,
        };

        match self.send_bug_report(report).await {
            Ok(receipt) => {
                tracing::info!(
                    public_id = %receipt.public_id,
                    "auto error report submitted"
                );
            }
            Err(e) => {
                tracing::warn!(error = %e, context = error_context, "auto error report submission failed");
            }
        }
    }
}

async fn export_to_endpoint(
    endpoint: &OtelEndpoint,
    spans: Vec<opentelemetry_sdk::trace::SpanData>,
) -> EndpointExportResult {
    let exporter = otel::build_exporter(endpoint);
    match exporter {
        Ok(otlp) => match otlp.exporter.export(spans).await {
            Ok(()) => {
                tracing::debug!(url = %endpoint.url, "trace export succeeded");
                EndpointExportResult {
                    url: endpoint.url.clone(),
                    success: true,
                    error: None,
                }
            }
            Err(e) => {
                let error = otlp.transport.describe_failure(&e);
                tracing::warn!(
                    url = %endpoint.url,
                    attempts = otlp.transport.attempts(),
                    error = %error,
                    "trace export failed"
                );
                EndpointExportResult {
                    url: endpoint.url.clone(),
                    success: false,
                    error: Some(error),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limiter_admits_first_report_for_a_context() {
        let mut limiter = ErrorReportLimiter::new();
        assert!(limiter.admit("boom", Instant::now()));
    }

    #[test]
    fn limiter_blocks_repeat_of_same_context_within_a_minute() {
        let mut limiter = ErrorReportLimiter::new();
        let t0 = Instant::now();
        assert!(limiter.admit("boom", t0));
        assert!(!limiter.admit("boom", t0 + Duration::from_secs(30)));
    }

    #[test]
    fn limiter_readmits_same_context_after_the_per_key_interval() {
        let mut limiter = ErrorReportLimiter::new();
        let t0 = Instant::now();
        assert!(limiter.admit("boom", t0));
        assert!(limiter.admit("boom", t0 + Duration::from_secs(61)));
    }

    #[test]
    fn limiter_treats_distinct_contexts_independently() {
        let mut limiter = ErrorReportLimiter::new();
        let t0 = Instant::now();
        assert!(limiter.admit("boom", t0));
        assert!(limiter.admit("crash", t0));
    }

    #[test]
    fn limiter_caps_total_admissions_per_hour_across_contexts() {
        let mut limiter = ErrorReportLimiter::new();
        let t0 = Instant::now();
        for i in 0..ErrorReportLimiter::HOURLY_CAP {
            assert!(
                limiter.admit(&format!("err-{i}"), t0),
                "admission {i} should succeed"
            );
        }
        assert!(
            !limiter.admit("one-too-many", t0),
            "6th distinct context within the hour must be suppressed by the hourly cap"
        );
    }

    #[test]
    fn limiter_hourly_cap_resets_after_the_window_elapses() {
        let mut limiter = ErrorReportLimiter::new();
        let t0 = Instant::now();
        for i in 0..ErrorReportLimiter::HOURLY_CAP {
            assert!(limiter.admit(&format!("err-{i}"), t0));
        }
        assert!(!limiter.admit("still-blocked", t0));
        assert!(limiter.admit(
            "after-window",
            t0 + ErrorReportLimiter::HOURLY_WINDOW + Duration::from_secs(1)
        ));
    }
}
