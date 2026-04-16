//! Wire-format serialization and HTTP transport for bug-report and
//! feedback submissions.
//!
//! The wire contract lives in `feedback-ingest/src/types.rs` and uses
//! `#[serde(deny_unknown_fields)]` on both request structs. Anything
//! emitted from this module must match those types byte-for-byte after
//! serde, or the upstream rejects with 422.

use anyhow::{Context, Result};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use opentelemetry_sdk::trace::SpanData;
use serde::Serialize;

use crate::util::telemetry::CompletedSpan;

use super::{
    BugReport, ClientContext, Feedback, FeedbackClient, Severity, SubmissionReceipt, otel,
    sanitize_spans,
};

/// Soft cap on the encoded OTLP payload, leaving headroom for base64
/// expansion and the JSON envelope under the relay's 8 MB body limit.
const PAYLOAD_BUDGET_BYTES: usize = 4 * 1024 * 1024;

/// Per-submission HTTP timeout. Matches the existing CLI bug-report timeout.
const SUBMISSION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Submit a bug report to `{endpoint}/api/v1/bug-report`.
///
/// Forces sanitization on the snapshot regardless of the runtime
/// `sanitize_content` toggle, trims the OTLP batch to stay under the
/// payload budget, encodes to protobuf, and base64-wraps the payload
/// before posting.
///
/// # Errors
/// Returns an error if the request fails to send, the upstream returns
/// a non-2xx status, or the response body doesn't decode into a
/// [`SubmissionReceipt`].
pub(super) async fn submit_bug_report(
    endpoint: &str,
    report: BugReport,
    spans: Vec<CompletedSpan>,
) -> Result<SubmissionReceipt> {
    let payload_bytes = encode_spans_for_submission(spans);
    let spans_b64 = BASE64_STANDARD.encode(&payload_bytes);

    let body = BugReportWire {
        kind: "bug",
        what_happened: &report.what_happened,
        what_expected: &report.what_expected,
        what_doing: &report.what_doing,
        severity: report.severity,
        client: &report.client,
        spans_otlp_b64: spans_b64,
    };

    let url = format!("{}/api/v1/bug-report", endpoint.trim_end_matches('/'));
    post_for_receipt(&url, &body).await
}

/// Submit a feedback message to `{endpoint}/api/v1/feedback`.
///
/// # Errors
/// Returns an error if the request fails to send, the upstream returns
/// a non-2xx status, or the response body doesn't decode into a
/// [`SubmissionReceipt`].
pub(super) async fn submit_feedback(
    endpoint: &str,
    feedback: Feedback,
) -> Result<SubmissionReceipt> {
    let body = FeedbackWire {
        kind: "feedback",
        message: &feedback.message,
        category: feedback.category.as_deref(),
        client: &feedback.client,
    };

    let url = format!("{}/api/v1/feedback", endpoint.trim_end_matches('/'));
    post_for_receipt(&url, &body).await
}

/// Sanitize, convert, trim, and OTLP-protobuf-encode the span buffer.
fn encode_spans_for_submission(spans: Vec<CompletedSpan>) -> Vec<u8> {
    let mut spans = spans;
    if spans.is_empty() {
        tracing::warn!(
            "span buffer empty at bug report submission; report will have no trace context"
        );
    }
    sanitize_spans(&mut spans);

    let otel_spans = otel::convert_spans(&spans);
    let trimmed = trim_spans_to_budget(otel_spans);
    otel::encode_otlp_protobuf(trimmed)
}

/// Trim spans from the front of the batch (oldest first) until the
/// encoded OTLP payload fits within [`PAYLOAD_BUDGET_BYTES`].
///
/// Returns the kept spans. Logs a warning when any are dropped.
fn trim_spans_to_budget(mut spans: Vec<SpanData>) -> Vec<SpanData> {
    if spans.is_empty() {
        return spans;
    }

    let original_count = spans.len();
    let mut encoded_size = otel::encode_otlp_protobuf(spans.clone()).len();

    // Iteratively drop the oldest 25% (or 1) until under budget. Capped
    // to avoid pathological runaway on huge buffers.
    for _ in 0_i32..16_i32 {
        if encoded_size <= PAYLOAD_BUDGET_BYTES {
            break;
        }
        let drop = (spans.len() / 4).max(1);
        let drop = drop.min(spans.len());
        spans.drain(..drop);
        if spans.is_empty() {
            break;
        }
        encoded_size = otel::encode_otlp_protobuf(spans.clone()).len();
    }

    let kept = spans.len();
    if kept < original_count {
        tracing::warn!(
            dropped = original_count - kept,
            kept,
            "trimmed span buffer to stay under payload limit"
        );
    }
    spans
}

/// Build a reqwest client with the standard submission timeout.
fn build_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(SUBMISSION_TIMEOUT)
        .build()
        .context("failed to build HTTP client for feedback submission")
}

/// POST a JSON body and decode the response as a [`SubmissionReceipt`].
///
/// Maps non-2xx responses to descriptive errors. For 429, the
/// upstream's `Retry-After` header is included verbatim in the message.
async fn post_for_receipt<B: Serialize>(url: &str, body: &B) -> Result<SubmissionReceipt> {
    let client = build_client()?;
    let resp = client
        .post(url)
        .json(body)
        .send()
        .await
        .with_context(|| format!("failed to reach upstream at {url}"))?;

    let status = resp.status();
    if !status.is_success() {
        let retry_after = resp
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let body_text = resp.text().await.unwrap_or_default();

        let mut msg = match retry_after {
            Some(ra) => format!("upstream returned {status} (retry after {ra}s)"),
            None => format!("upstream returned {status}"),
        };
        if !body_text.is_empty() {
            // The wire contract is `{ "error": "<terse>" }`; surface the
            // server's error string when present, otherwise dump raw.
            if let Ok(parsed) = serde_json::from_str::<UpstreamError>(&body_text) {
                msg.push_str(": ");
                msg.push_str(&parsed.error);
            } else {
                msg.push_str(": ");
                msg.push_str(&body_text);
            }
        }
        anyhow::bail!(msg);
    }

    resp.json::<SubmissionReceipt>()
        .await
        .context("upstream returned an unexpected response body")
}

#[derive(serde::Deserialize)]
struct UpstreamError {
    error: String,
}

#[derive(Serialize)]
struct BugReportWire<'a> {
    kind: &'static str,
    what_happened: &'a str,
    what_expected: &'a str,
    what_doing: &'a str,
    severity: Severity,
    client: &'a ClientContext,
    spans_otlp_b64: String,
}

#[derive(Serialize)]
struct FeedbackWire<'a> {
    kind: &'static str,
    message: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    category: Option<&'a str>,
    client: &'a FeedbackClient,
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
#[expect(
    clippy::indexing_slicing,
    reason = "test code indexes known-length slices"
)]
mod tests {
    use super::*;

    fn sample_client() -> ClientContext {
        ClientContext {
            version: "v2026.04.16".to_string(),
            commit: Some("abc123def456".to_string()),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            model_provider: Some("anthropic".to_string()),
            model_name: Some("claude-opus-4-7".to_string()),
            active_subagents: Vec::new(),
            config_flags: std::collections::BTreeMap::new(),
        }
    }

    #[test]
    fn bug_report_wire_serializes_with_kind_tag_and_lowercase_severity() {
        let wire = BugReportWire {
            kind: "bug",
            what_happened: "it crashed",
            what_expected: "it should not crash",
            what_doing: "running the thing",
            severity: Severity::Broken,
            client: &sample_client(),
            spans_otlp_b64: "aGVsbG8=".to_string(),
        };
        let json: serde_json::Value = serde_json::to_value(&wire).unwrap();
        assert_eq!(json["kind"], "bug");
        assert_eq!(json["severity"], "broken");
        assert_eq!(json["spans_otlp_b64"], "aGVsbG8=");
        assert_eq!(json["client"]["os"], "linux");
        assert_eq!(
            json["client"]["active_subagents"].as_array().unwrap().len(),
            0
        );
    }

    #[test]
    fn feedback_wire_omits_category_when_none() {
        let client = FeedbackClient {
            version: "v2026.04.16".to_string(),
        };
        let wire = FeedbackWire {
            kind: "feedback",
            message: "thoughts",
            category: None,
            client: &client,
        };
        let json: serde_json::Value = serde_json::to_value(&wire).unwrap();
        assert_eq!(json["kind"], "feedback");
        assert_eq!(json["message"], "thoughts");
        assert!(
            json.get("category").is_none(),
            "category must be omitted when None"
        );
        // Feedback client carries version only — no other fields leak through.
        assert_eq!(json["client"].as_object().unwrap().len(), 1);
        assert_eq!(json["client"]["version"], "v2026.04.16");
    }

    #[test]
    fn feedback_wire_includes_category_when_some() {
        let client = FeedbackClient {
            version: "v2026.04.16".to_string(),
        };
        let wire = FeedbackWire {
            kind: "feedback",
            message: "thoughts",
            category: Some("ui"),
            client: &client,
        };
        let json: serde_json::Value = serde_json::to_value(&wire).unwrap();
        assert_eq!(json["category"], "ui");
    }

    #[test]
    fn severity_serializes_lowercase() {
        assert_eq!(serde_json::to_value(Severity::Broken).unwrap(), "broken");
        assert_eq!(serde_json::to_value(Severity::Wrong).unwrap(), "wrong");
        assert_eq!(
            serde_json::to_value(Severity::Annoying).unwrap(),
            "annoying"
        );
    }

    #[test]
    fn trim_spans_returns_empty_unchanged() {
        let trimmed = trim_spans_to_budget(Vec::new());
        assert!(trimmed.is_empty());
    }
}
