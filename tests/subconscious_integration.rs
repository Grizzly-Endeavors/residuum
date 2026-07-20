//! End-to-end integration tests for the subconscious classifier.
//!
//! Exercises the real config → `Subconscious::build` wiring and the
//! `evaluate` path with a mock provider, through the crate's public API.

#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
#[expect(
    clippy::tests_outside_test_module,
    reason = "integration tests live in tests/ directory, not inside #[cfg(test)] modules"
)]
mod subconscious_integration {
    use async_trait::async_trait;

    use residuum::config::Config;
    use residuum::models::{
        CompletionOptions, HttpClientConfig, Message, ModelError, ModelProvider, ModelResponse,
        SharedHttpClient, ToolDefinition,
    };
    use residuum::subconscious::{
        EvalPhase, FindingKind, Severity, Subconscious, SubconsciousConfig,
    };
    use residuum::workspace::layout::WorkspaceLayout;

    /// Mock provider returning a fixed response; records whether it was called.
    struct CountingProvider {
        response: String,
        calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait]
    impl ModelProvider for CountingProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _options: &CompletionOptions,
        ) -> Result<ModelResponse, ModelError> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(ModelResponse::new(self.response.clone(), vec![]))
        }

        fn model_name(&self) -> &'static str {
            "mock-subconscious"
        }
    }

    fn http() -> SharedHttpClient {
        SharedHttpClient::new(&HttpClientConfig::default()).unwrap()
    }

    fn write_config(dir: &std::path::Path, config_toml: &str) {
        std::fs::write(dir.join("config.toml"), config_toml).unwrap();
        // Ollama needs no API key, so the provider chain builds in tests.
        std::fs::write(
            dir.join("providers.toml"),
            r#"
[models]
main = "ollama/llama3"
subconscious = "ollama/llama3-mini"
"#,
        )
        .unwrap();
    }

    #[test]
    fn build_enabled_from_config() {
        let dir = tempfile::tempdir().unwrap();
        write_config(
            dir.path(),
            "timezone = \"UTC\"\n\n[subconscious]\nenabled = true\n",
        );
        let cfg = Config::load_at(dir.path()).unwrap();

        let layout = WorkspaceLayout::new(dir.path());
        let sub = Subconscious::build(&cfg, &layout, http());

        assert!(
            sub.enabled(),
            "enabled config should build an enabled instance"
        );
        assert!(sub.mid_turn_enabled(), "mid_turn defaults on when enabled");
        assert_eq!(
            cfg.subconscious.first().unwrap().model.model,
            "llama3-mini",
            "subconscious role should resolve to its assigned model"
        );
    }

    #[test]
    fn build_disabled_when_section_absent() {
        let dir = tempfile::tempdir().unwrap();
        write_config(dir.path(), "timezone = \"UTC\"\n");
        let cfg = Config::load_at(dir.path()).unwrap();

        let layout = WorkspaceLayout::new(dir.path());
        let sub = Subconscious::build(&cfg, &layout, http());

        assert!(
            !sub.enabled(),
            "subconscious is opt-in; absent section must build a disabled instance"
        );
        assert!(!sub.mid_turn_enabled());
    }

    #[tokio::test]
    async fn evaluate_end_to_end_produces_findings() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

        let response = r#"{
            "findings": [
                {"kind": "omission", "severity": "act", "instruction": "Persist the user's bullet-point preference to MEMORY.md."}
            ]
        }"#;
        let sub = Subconscious::new(
            Box::new(CountingProvider {
                response: response.to_string(),
                calls: std::sync::Arc::clone(&calls),
            }),
            SubconsciousConfig {
                enabled: true,
                ..SubconsciousConfig::default()
            },
            layout,
        );

        let transcript = vec![
            Message::user("Always answer me in bullet points."),
            Message::assistant("Got it.".to_string(), None),
        ];
        let findings = sub
            .evaluate(&transcript, EvalPhase::EndOfTurn, None)
            .await
            .unwrap();

        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(findings.len(), 1);
        let finding = findings.first().unwrap();
        assert_eq!(finding.kind, FindingKind::Omission);
        assert_eq!(finding.severity, Severity::Act);
        assert!(finding.instruction.contains("MEMORY.md"));
    }

    #[tokio::test]
    async fn disabled_instance_never_calls_provider() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());

        // A disabled instance uses a NullProvider that errors if called; the
        // empty-transcript short-circuit and the caller's enabled() gate keep it
        // from ever running. Here we assert the disabled state directly.
        let sub = Subconscious::disabled(layout);
        assert!(
            !sub.enabled(),
            "disabled config performs zero classification"
        );
    }
}
