//! End-to-end integration tests for the proactivity subsystem (Phase 3).
//!
//! Tests the pulse and cron systems using mock providers and temporary workspaces.

#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
#[expect(
    clippy::panic,
    reason = "test assertions use panic for unreachable variants"
)]
#[expect(
    clippy::tests_outside_test_module,
    reason = "integration tests live in tests/ directory, not inside #[cfg(test)] modules"
)]
mod proactivity_integration {
    use async_trait::async_trait;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::tempdir;

    use ironclaw::agent::Agent;
    use ironclaw::agent::context::{ProjectsContext, SkillsContext};
    use ironclaw::background::types::{Execution, ResultRouting};
    use ironclaw::channels::null::NullDisplay;
    use ironclaw::config::BackgroundModelTier;
    use ironclaw::models::{
        CompletionOptions, Message, ModelError, ModelProvider, ModelResponse, Role, ToolDefinition,
    };
    use ironclaw::notify::types::TaskSource;
    use ironclaw::pulse::executor::build_pulse_task;
    use ironclaw::pulse::scheduler::PulseScheduler;
    use ironclaw::pulse::types::{PulseDef, PulseTask};
    use ironclaw::tools::{ToolFilter, ToolRegistry};
    use ironclaw::workspace::identity::IdentityFiles;

    /// Mock provider that returns configurable responses in sequence.
    struct MockProvider {
        responses: Vec<String>,
        call_idx: Arc<AtomicUsize>,
    }

    impl MockProvider {
        fn new(responses: Vec<String>) -> Self {
            Self {
                responses,
                call_idx: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    #[async_trait]
    impl ModelProvider for MockProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _options: &CompletionOptions,
        ) -> Result<ModelResponse, ModelError> {
            let idx = self.call_idx.fetch_add(1, Ordering::SeqCst);
            let content = self
                .responses
                .get(idx)
                .cloned()
                .unwrap_or_else(|| self.responses.last().cloned().unwrap_or_default());
            Ok(ModelResponse::new(content, vec![]))
        }

        fn model_name(&self) -> &'static str {
            "mock-proactivity"
        }
    }

    fn make_agent(responses: Vec<String>) -> Agent {
        Agent::new(
            Box::new(MockProvider::new(responses)),
            ToolRegistry::new(),
            ToolFilter::new_shared(std::collections::HashSet::new()),
            ironclaw::mcp::McpRegistry::new_shared(),
            IdentityFiles::default(),
            CompletionOptions::default(),
            chrono_tz::UTC,
            std::path::PathBuf::from("/tmp/ironclaw-test-inbox"),
        )
    }

    fn sample_pulse() -> PulseDef {
        PulseDef {
            name: "email_check".to_string(),
            enabled: true,
            schedule: "30m".to_string(),
            active_hours: None,
            tasks: vec![PulseTask {
                name: "check_inbox".to_string(),
                prompt: "Check email.".to_string(),
            }],
        }
    }

    // ── build_pulse_task tests ───────────────────────────────────────────

    #[test]
    fn build_pulse_task_source_is_pulse() {
        let pulse = sample_pulse();
        let task = build_pulse_task(&pulse);
        assert!(
            matches!(task.source, TaskSource::Pulse),
            "source should be Pulse"
        );
    }

    #[test]
    fn build_pulse_task_name_matches_pulse() {
        let pulse = sample_pulse();
        let task = build_pulse_task(&pulse);
        assert_eq!(
            task.task_name, "email_check",
            "task_name should match pulse name"
        );
    }

    #[test]
    fn build_pulse_task_id_format() {
        let pulse = sample_pulse();
        let task = build_pulse_task(&pulse);
        assert!(
            task.id.starts_with("pulse-email_check-"),
            "id should start with pulse-<name>-"
        );
    }

    #[test]
    fn build_pulse_task_execution_is_subagent_small() {
        let pulse = sample_pulse();
        let task = build_pulse_task(&pulse);
        match &task.execution {
            Execution::SubAgent(cfg) => {
                assert_eq!(
                    cfg.model_tier,
                    BackgroundModelTier::Small,
                    "tier should be Small"
                );
            }
            Execution::Script(_) => panic!("expected SubAgent execution"),
        }
    }

    #[test]
    fn build_pulse_task_prompt_contains_pulse_name_and_heartbeat_ok() {
        let pulse = sample_pulse();
        let task = build_pulse_task(&pulse);
        let prompt = match &task.execution {
            Execution::SubAgent(cfg) => &cfg.prompt,
            Execution::Script(_) => panic!("expected SubAgent"),
        };

        assert!(
            prompt.contains("email_check"),
            "prompt should contain pulse name"
        );
        assert!(
            prompt.contains("check_inbox"),
            "prompt should contain task name"
        );
        assert!(
            prompt.contains("Check email"),
            "prompt should contain task prompt"
        );
        assert!(
            prompt.contains("HEARTBEAT_OK"),
            "prompt should contain HEARTBEAT_OK instruction"
        );
    }

    #[test]
    fn build_pulse_task_routing_is_notify() {
        let pulse = sample_pulse();
        let task = build_pulse_task(&pulse);
        assert!(
            matches!(task.routing, ResultRouting::Notify),
            "routing should be Notify"
        );
    }

    #[test]
    fn build_pulse_task_with_no_tasks() {
        let pulse = PulseDef {
            name: "empty_test".to_string(),
            enabled: true,
            schedule: "1h".to_string(),
            active_hours: None,
            tasks: vec![],
        };

        let task = build_pulse_task(&pulse);
        assert_eq!(task.task_name, "empty_test");
        let prompt = match &task.execution {
            Execution::SubAgent(cfg) => &cfg.prompt,
            Execution::Script(_) => panic!("expected SubAgent"),
        };
        assert!(
            prompt.contains("HEARTBEAT_OK"),
            "should still have HEARTBEAT_OK instruction with no tasks"
        );
    }

    // ── Scheduler tests ──────────────────────────────────────────────────────

    #[test]
    fn scheduler_due_on_first_run() {
        let dir = tempdir().unwrap();
        let heartbeat_path = dir.path().join("HEARTBEAT.yml");
        std::fs::write(
            &heartbeat_path,
            "pulses:\n  - name: p1\n    schedule: \"1h\"\n    tasks: []",
        )
        .unwrap();

        let mut scheduler = PulseScheduler::new();
        let now = chrono::Utc::now().naive_utc();
        let due = scheduler.due_pulses(now, &heartbeat_path);
        assert_eq!(due.len(), 1, "pulse should fire on first run");
    }

    #[test]
    fn scheduler_does_not_refire_immediately() {
        let dir = tempdir().unwrap();
        let heartbeat_path = dir.path().join("HEARTBEAT.yml");
        std::fs::write(
            &heartbeat_path,
            "pulses:\n  - name: p1\n    schedule: \"2h\"\n    tasks: []",
        )
        .unwrap();

        let mut scheduler = PulseScheduler::new();
        let now = chrono::Utc::now().naive_utc();

        let first = scheduler.due_pulses(now, &heartbeat_path);
        assert_eq!(first.len(), 1, "first call should fire");

        let second = scheduler.due_pulses(now, &heartbeat_path);
        assert!(second.is_empty(), "same-time call should not refire");
    }

    // ── Cron store tests ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn cron_store_round_trip() {
        use ironclaw::cron::store::CronStore;
        use ironclaw::cron::types::{CronJob, CronJobState, CronPayload, CronSchedule};

        let dir = tempdir().unwrap();
        let path = dir.path().join("jobs.json");

        let now = chrono::Utc::now().naive_utc();
        let job = CronJob {
            id: "cron-test0001".to_string(),
            name: "test job".to_string(),
            description: Some("integration test job".to_string()),
            enabled: true,
            delete_after_run: false,
            created_at: now,
            updated_at: now,
            schedule: CronSchedule::Every {
                every_ms: 3_600_000,
                anchor_ms: 0,
            },
            payload: CronPayload::AgentTurn {
                message: "Run a check.".to_string(),
            },
            state: CronJobState::default(),
        };

        let mut store = CronStore::load(&path).await.unwrap();
        store.add_job(job);
        store.save().await.unwrap();

        let reloaded = CronStore::load(&path).await.unwrap();
        assert_eq!(
            reloaded.list_jobs().len(),
            1,
            "should have one job after reload"
        );

        let loaded_job = reloaded.list_jobs().first().unwrap();
        assert_eq!(
            loaded_job.id, "cron-test0001",
            "job id should survive reload"
        );
        assert_eq!(loaded_job.name, "test job", "name should survive reload");
        assert_eq!(
            loaded_job.description.as_deref(),
            Some("integration test job"),
            "description should survive reload"
        );
    }

    // ── run_system_turn tests ────────────────────────────────────────────────

    #[tokio::test]
    async fn run_system_turn_does_not_modify_main_history() {
        let agent = make_agent(vec!["I ran a background check.".to_string()]);
        let display = NullDisplay;

        let no_projects = ProjectsContext::none();
        let result = agent
            .run_system_turn(
                "background check prompt",
                &display,
                None,
                &no_projects,
                &SkillsContext::none(),
            )
            .await
            .unwrap();

        assert_eq!(
            result.response, "I ran a background check.",
            "response should match mock"
        );
        assert_eq!(
            agent.message_count(),
            0,
            "main message history should be empty"
        );
        assert!(
            !result.messages.is_empty(),
            "should have ephemeral messages"
        );

        // Ephemeral messages include at least the user prompt and assistant response
        let has_user = result.messages.iter().any(|m| m.role == Role::User);
        let has_assistant = result.messages.iter().any(|m| m.role == Role::Assistant);
        assert!(has_user, "should have user message in thread");
        assert!(has_assistant, "should have assistant message in thread");
    }
}
