//! End-to-end tests for the feedback / bug-report HTTP submission path.
//!
//! Verifies that `TracingService::send_bug_report` and `send_feedback`
//! produce wire-compatible JSON, decode the documented receipt shape,
//! and surface upstream errors (including 429 Retry-After) to the caller.

#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
#[expect(clippy::expect_used, reason = "test code uses expect for clarity")]
#[expect(
    clippy::indexing_slicing,
    reason = "test code indexes serde_json::Value by known-present keys"
)]
#[expect(
    clippy::tests_outside_test_module,
    reason = "integration tests live in tests/ directory, not inside #[cfg(test)] modules"
)]
mod feedback_submit_integration {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use residuum::config::TracingConfig;
    use residuum::tracing_service::{
        BugReport, ClientContext, Feedback, FeedbackClient, Severity, TracingService,
    };
    use residuum::util::telemetry::{SpanBufferConfig, SpanBufferLayer};
    use serde_json::Value;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, Request, ResponseTemplate};

    fn sample_client() -> ClientContext {
        ClientContext {
            version: "v2026.04.16".to_string(),
            commit: Some("deadbeef1234".to_string()),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            model_provider: Some("anthropic".to_string()),
            model_name: Some("claude-opus-4-7".to_string()),
            active_subagents: Vec::new(),
            config_flags: BTreeMap::new(),
        }
    }

    fn build_service(endpoint: &str) -> Arc<TracingService> {
        let cfg = TracingConfig {
            feedback_endpoint: endpoint.to_string(),
            ..TracingConfig::default()
        };
        let (_, handle) = SpanBufferLayer::new(&SpanBufferConfig::default());
        Arc::new(TracingService::new(cfg, handle))
    }

    /// Captured request body: shared between the mock responder and the test
    /// assertions. Created fresh per test.
    fn capture_body() -> Arc<std::sync::Mutex<Option<Value>>> {
        Arc::new(std::sync::Mutex::new(None))
    }

    #[tokio::test]
    async fn bug_report_happy_path_sends_wire_contract_and_returns_receipt() {
        let server = MockServer::start().await;
        let captured = capture_body();
        let captured_for_mock = Arc::clone(&captured);

        Mock::given(method("POST"))
            .and(path("/api/v1/bug-report"))
            .and(header("content-type", "application/json"))
            .respond_with(move |req: &Request| {
                let body: Value = serde_json::from_slice(&req.body).unwrap();
                *captured_for_mock.lock().unwrap() = Some(body);
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "public_id": "RR-01HZKX2P7M",
                    "submitted_at": "2026-04-16T14:23:00Z"
                }))
            })
            .mount(&server)
            .await;

        let service = build_service(&server.uri());
        let receipt = service
            .send_bug_report(BugReport {
                what_happened: "it crashed".to_string(),
                what_expected: "it should not crash".to_string(),
                what_doing: "running the thing".to_string(),
                severity: Severity::Broken,
                client: sample_client(),
            })
            .await
            .expect("submission must succeed");

        assert_eq!(receipt.public_id, "RR-01HZKX2P7M");
        assert_eq!(receipt.submitted_at, "2026-04-16T14:23:00Z");

        let body = captured
            .lock()
            .unwrap()
            .clone()
            .expect("mock must have captured a body");
        assert_eq!(body["kind"], "bug", "wire-format `kind` tag must be 'bug'");
        assert_eq!(body["what_happened"], "it crashed");
        assert_eq!(body["what_expected"], "it should not crash");
        assert_eq!(body["what_doing"], "running the thing");
        assert_eq!(body["severity"], "broken", "severity must lowercase");
        assert!(
            body["spans_otlp_b64"].is_string(),
            "spans_otlp_b64 is required even when the span buffer is empty"
        );
        assert_eq!(body["client"]["version"], "v2026.04.16");
        assert_eq!(body["client"]["commit"], "deadbeef1234");
        assert_eq!(body["client"]["model_provider"], "anthropic");
        assert!(
            body["client"]["active_subagents"].is_array()
                && body["client"]["active_subagents"]
                    .as_array()
                    .unwrap()
                    .is_empty(),
            "active_subagents must be present and empty"
        );
        assert!(
            body["client"]["config_flags"].is_object()
                && body["client"]["config_flags"]
                    .as_object()
                    .unwrap()
                    .is_empty(),
            "config_flags must be present and empty"
        );
    }

    #[tokio::test]
    async fn feedback_happy_path_sends_version_only_client() {
        let server = MockServer::start().await;
        let captured = capture_body();
        let captured_for_mock = Arc::clone(&captured);

        Mock::given(method("POST"))
            .and(path("/api/v1/feedback"))
            .respond_with(move |req: &Request| {
                let body: Value = serde_json::from_slice(&req.body).unwrap();
                *captured_for_mock.lock().unwrap() = Some(body);
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "public_id": "RR-FBK99",
                    "submitted_at": "2026-04-16T15:00:00Z"
                }))
            })
            .mount(&server)
            .await;

        let service = build_service(&server.uri());
        let receipt = service
            .send_feedback(Feedback {
                message: "the chat history reload is slick".to_string(),
                category: Some("ui".to_string()),
                client: FeedbackClient {
                    version: "v2026.04.16".to_string(),
                },
            })
            .await
            .expect("submission must succeed");
        assert_eq!(receipt.public_id, "RR-FBK99");

        let body = captured.lock().unwrap().clone().unwrap();
        assert_eq!(body["kind"], "feedback");
        assert_eq!(body["message"], "the chat history reload is slick");
        assert_eq!(body["category"], "ui");
        let client_obj = body["client"].as_object().unwrap();
        assert_eq!(
            client_obj.len(),
            1,
            "feedback client must carry version only — got {:?}",
            client_obj.keys().collect::<Vec<_>>()
        );
        assert_eq!(body["client"]["version"], "v2026.04.16");
    }

    #[tokio::test]
    async fn feedback_omits_category_when_none() {
        let server = MockServer::start().await;
        let captured = capture_body();
        let captured_for_mock = Arc::clone(&captured);

        Mock::given(method("POST"))
            .and(path("/api/v1/feedback"))
            .respond_with(move |req: &Request| {
                let body: Value = serde_json::from_slice(&req.body).unwrap();
                *captured_for_mock.lock().unwrap() = Some(body);
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "public_id": "RR-X",
                    "submitted_at": "2026-04-16T15:00:00Z"
                }))
            })
            .mount(&server)
            .await;

        let service = build_service(&server.uri());
        service
            .send_feedback(Feedback {
                message: "no category here".to_string(),
                category: None,
                client: FeedbackClient {
                    version: "v2026.04.16".to_string(),
                },
            })
            .await
            .unwrap();

        let body = captured.lock().unwrap().clone().unwrap();
        assert!(
            body.as_object().unwrap().get("category").is_none(),
            "category must be omitted when None to keep the JSON envelope minimal"
        );
    }

    #[tokio::test]
    async fn upstream_422_surfaces_error_body_to_caller() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/bug-report"))
            .respond_with(ResponseTemplate::new(422).set_body_json(
                serde_json::json!({"error": "severity must be one of broken|wrong|annoying"}),
            ))
            .mount(&server)
            .await;

        let service = build_service(&server.uri());
        let err = service
            .send_bug_report(BugReport {
                what_happened: "x".to_string(),
                what_expected: "y".to_string(),
                what_doing: "z".to_string(),
                severity: Severity::Broken,
                client: sample_client(),
            })
            .await
            .expect_err("422 must error");
        let msg = format!("{err}");
        assert!(msg.contains("422"), "error must include status: {msg}");
        assert!(
            msg.contains("severity must be one of"),
            "error must surface upstream message: {msg}"
        );
    }

    #[tokio::test]
    async fn upstream_429_surfaces_retry_after_header() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/feedback"))
            .respond_with(
                ResponseTemplate::new(429)
                    .insert_header("retry-after", "120")
                    .set_body_json(serde_json::json!({"error": "rate limited"})),
            )
            .mount(&server)
            .await;

        let service = build_service(&server.uri());
        let err = service
            .send_feedback(Feedback {
                message: "test".to_string(),
                category: None,
                client: FeedbackClient {
                    version: "v2026.04.16".to_string(),
                },
            })
            .await
            .expect_err("429 must error");
        let msg = format!("{err}");
        assert!(
            msg.contains("retry after 120s"),
            "Retry-After must be surfaced in the error: {msg}"
        );
    }
}
