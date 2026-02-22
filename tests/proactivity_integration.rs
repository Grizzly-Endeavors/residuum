//! End-to-end integration tests for the proactivity subsystem (Phase 3).
//!
//! Tests the pulse and cron systems using mock providers and temporary workspaces.

#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
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
    use ironclaw::channels::null::NullDisplay;
    use ironclaw::models::{
        CompletionOptions, Message, ModelError, ModelProvider, ModelResponse, Role, ToolDefinition,
    };
    use ironclaw::pulse::executor::execute_pulse;
    use ironclaw::pulse::scheduler::PulseScheduler;
    use ironclaw::pulse::types::{AlertLevel, PulseDef, PulseTask};
    use ironclaw::tools::ToolRegistry;
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
            IdentityFiles::default(),
            CompletionOptions::default(),
            chrono_tz::UTC,
        )
    }

    // ── Pulse tests ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn pulse_heartbeat_ok_response() {
        let dir = tempdir().unwrap();
        let alerts_path = dir.path().join("Alerts.md");

        let agent = make_agent(vec!["HEARTBEAT_OK".to_string()]);

        let pulse = PulseDef {
            name: "test_pulse".to_string(),
            enabled: true,
            schedule: "1h".to_string(),
            active_hours: None,
            tasks: vec![PulseTask {
                name: "check".to_string(),
                prompt: "Check everything.".to_string(),
                alert: AlertLevel::Low,
            }],
        };

        let result = execute_pulse(&pulse, &agent, &alerts_path, None)
            .await
            .unwrap();

        assert!(
            result.is_heartbeat_ok,
            "response with HEARTBEAT_OK should set is_heartbeat_ok"
        );
        assert_eq!(result.pulse_name, "test_pulse", "pulse name should match");
        assert!(
            !result.messages.is_empty(),
            "should have ephemeral messages"
        );
        assert_eq!(
            agent.message_count(),
            0,
            "main message history should be untouched"
        );
    }

    #[tokio::test]
    async fn pulse_finding_response() {
        let dir = tempdir().unwrap();
        let alerts_path = dir.path().join("Alerts.md");

        let agent = make_agent(vec!["Found 3 urgent emails requiring action.".to_string()]);

        let pulse = PulseDef {
            name: "email_check".to_string(),
            enabled: true,
            schedule: "30m".to_string(),
            active_hours: None,
            tasks: vec![PulseTask {
                name: "check_inbox".to_string(),
                prompt: "Check email.".to_string(),
                alert: AlertLevel::High,
            }],
        };

        let result = execute_pulse(&pulse, &agent, &alerts_path, None)
            .await
            .unwrap();

        assert!(
            !result.is_heartbeat_ok,
            "non-HEARTBEAT_OK response should not set flag"
        );
        assert_eq!(result.pulse_name, "email_check", "pulse name should match");
    }

    #[tokio::test]
    async fn pulse_with_alerts_md_content() {
        let dir = tempdir().unwrap();
        let alerts_path = dir.path().join("Alerts.md");
        tokio::fs::write(&alerts_path, "# Alert Guidelines\nBe concise.")
            .await
            .unwrap();

        let agent = make_agent(vec!["HEARTBEAT_OK".to_string()]);

        let pulse = PulseDef {
            name: "alert_test".to_string(),
            enabled: true,
            schedule: "1h".to_string(),
            active_hours: None,
            tasks: vec![],
        };

        let result = execute_pulse(&pulse, &agent, &alerts_path, None)
            .await
            .unwrap();
        assert!(
            result.is_heartbeat_ok,
            "should still process with alerts.md present"
        );
    }

    #[tokio::test]
    async fn pulse_thread_messages_not_in_main_history() {
        let dir = tempdir().unwrap();
        let alerts_path = dir.path().join("Alerts.md");

        let agent = make_agent(vec!["HEARTBEAT_OK".to_string()]);

        let pulse = PulseDef {
            name: "ephemeral_test".to_string(),
            enabled: true,
            schedule: "1h".to_string(),
            active_hours: None,
            tasks: vec![PulseTask {
                name: "check".to_string(),
                prompt: "Check something.".to_string(),
                alert: AlertLevel::Medium,
            }],
        };

        let result = execute_pulse(&pulse, &agent, &alerts_path, None)
            .await
            .unwrap();

        // Ephemeral messages returned for memory pipeline
        assert!(
            !result.messages.is_empty(),
            "should have ephemeral messages"
        );
        // Main agent message history untouched
        assert_eq!(
            agent.message_count(),
            0,
            "main agent message history should be empty"
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
        use ironclaw::cron::types::{CronJob, CronJobState, CronPayload, CronSchedule, Delivery};

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
            delivery: Delivery::Background,
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

    #[tokio::test]
    async fn cron_executor_system_event_queues_on_agent() {
        use ironclaw::cron::executor::execute_due_jobs;
        use ironclaw::cron::store::CronStore;
        use ironclaw::cron::types::{CronJob, CronJobState, CronPayload, CronSchedule, Delivery};

        let dir = tempdir().unwrap();
        let path = dir.path().join("jobs.json");

        let now = chrono::Utc::now().naive_utc();
        let past = now - chrono::Duration::seconds(10);

        let job = CronJob {
            id: "cron-evt00001".to_string(),
            name: "event job".to_string(),
            description: None,
            enabled: true,
            delete_after_run: false,
            created_at: now,
            updated_at: now,
            schedule: CronSchedule::At { at: past },
            delivery: Delivery::UserVisible,
            payload: CronPayload::SystemEvent {
                text: "system alert text".to_string(),
            },
            state: CronJobState {
                next_run_at: Some(past),
                ..CronJobState::default()
            },
        };

        let mut store = CronStore::load(&path).await.unwrap();
        store.add_job(job);

        let mut agent = make_agent(vec![]);

        // Initially no pending events
        assert_eq!(agent.message_count(), 0, "no messages initially");

        let result = execute_due_jobs(&mut store, &mut agent, now, chrono_tz::UTC, None)
            .await
            .unwrap();

        // SystemEvent+Main produces no ephemeral messages (just queues on agent)
        assert!(
            result.messages.is_empty(),
            "system event should produce no ephemeral messages"
        );
        // But produces a notification for display
        assert_eq!(
            result.notifications.len(),
            1,
            "should produce one notification"
        );
    }

    #[tokio::test]
    async fn cron_executor_agent_turn_returns_messages() {
        use ironclaw::cron::executor::execute_due_jobs;
        use ironclaw::cron::store::CronStore;
        use ironclaw::cron::types::{CronJob, CronJobState, CronPayload, CronSchedule, Delivery};

        let dir = tempdir().unwrap();
        let path = dir.path().join("jobs.json");

        let now = chrono::Utc::now().naive_utc();
        let past = now - chrono::Duration::seconds(10);

        let job = CronJob {
            id: "cron-agt00001".to_string(),
            name: "agent turn job".to_string(),
            description: None,
            enabled: true,
            delete_after_run: false,
            created_at: now,
            updated_at: now,
            schedule: CronSchedule::At { at: past },
            delivery: Delivery::Background,
            payload: CronPayload::AgentTurn {
                message: "Do a background check.".to_string(),
            },
            state: CronJobState {
                next_run_at: Some(past),
                ..CronJobState::default()
            },
        };

        let mut store = CronStore::load(&path).await.unwrap();
        store.add_job(job);

        let mut agent = make_agent(vec!["Background check complete.".to_string()]);

        let result = execute_due_jobs(&mut store, &mut agent, now, chrono_tz::UTC, None)
            .await
            .unwrap();

        // AgentTurn+Isolated returns ephemeral messages for memory pipeline
        assert!(
            !result.messages.is_empty(),
            "agent turn should produce ephemeral messages"
        );
        // Main agent message history should be untouched
        assert_eq!(
            agent.message_count(),
            0,
            "main message history should be untouched"
        );
    }

    // ── Cron delivery × payload matrix tests ───────────────────────────────

    #[tokio::test]
    async fn cron_executor_system_event_background_returns_messages() {
        use ironclaw::cron::executor::execute_due_jobs;
        use ironclaw::cron::store::CronStore;
        use ironclaw::cron::types::{CronJob, CronJobState, CronPayload, CronSchedule, Delivery};

        let dir = tempdir().unwrap();
        let path = dir.path().join("jobs.json");

        let now = chrono::Utc::now().naive_utc();
        let past = now - chrono::Duration::seconds(10);

        let job = CronJob {
            id: "cron-bg-evt01".to_string(),
            name: "bg event".to_string(),
            description: None,
            enabled: true,
            delete_after_run: false,
            created_at: now,
            updated_at: now,
            schedule: CronSchedule::At { at: past },
            delivery: Delivery::Background,
            payload: CronPayload::SystemEvent {
                text: "background alert".to_string(),
            },
            state: CronJobState {
                next_run_at: Some(past),
                ..CronJobState::default()
            },
        };

        let mut store = CronStore::load(&path).await.unwrap();
        store.add_job(job);

        let mut agent = make_agent(vec![]);

        let result = execute_due_jobs(&mut store, &mut agent, now, chrono_tz::UTC, None)
            .await
            .unwrap();

        assert!(
            !result.messages.is_empty(),
            "background system event should return messages for memory pipeline"
        );
        assert!(
            result
                .messages
                .first()
                .unwrap()
                .content
                .contains("background alert"),
            "synthetic message should contain event text"
        );
        assert_eq!(
            agent.message_count(),
            0,
            "main message history should be untouched"
        );
    }

    #[tokio::test]
    async fn cron_executor_agent_turn_user_visible_returns_messages() {
        use ironclaw::cron::executor::execute_due_jobs;
        use ironclaw::cron::store::CronStore;
        use ironclaw::cron::types::{CronJob, CronJobState, CronPayload, CronSchedule, Delivery};

        let dir = tempdir().unwrap();
        let path = dir.path().join("jobs.json");

        let now = chrono::Utc::now().naive_utc();
        let past = now - chrono::Duration::seconds(10);

        let job = CronJob {
            id: "cron-vis-agt1".to_string(),
            name: "visible agent".to_string(),
            description: None,
            enabled: true,
            delete_after_run: false,
            created_at: now,
            updated_at: now,
            schedule: CronSchedule::At { at: past },
            delivery: Delivery::UserVisible,
            payload: CronPayload::AgentTurn {
                message: "Do a visible check.".to_string(),
            },
            state: CronJobState {
                next_run_at: Some(past),
                ..CronJobState::default()
            },
        };

        let mut store = CronStore::load(&path).await.unwrap();
        store.add_job(job);

        let mut agent = make_agent(vec!["Visible check done.".to_string()]);

        let result = execute_due_jobs(&mut store, &mut agent, now, chrono_tz::UTC, None)
            .await
            .unwrap();

        // UserVisible agent turn should return ephemeral messages for memory
        assert!(
            !result.messages.is_empty(),
            "user-visible agent turn should return messages"
        );
        // Also produces a notification for display
        assert_eq!(
            result.notifications.len(),
            1,
            "should produce one notification"
        );
    }

    // ── run_system_turn tests ────────────────────────────────────────────────

    #[tokio::test]
    async fn run_system_turn_does_not_modify_main_history() {
        let agent = make_agent(vec!["I ran a background check.".to_string()]);
        let display = NullDisplay;

        let result = agent
            .run_system_turn("background check prompt", &display, None)
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
