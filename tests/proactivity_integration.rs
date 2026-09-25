//! End-to-end integration tests for the proactivity subsystem (Phase 3).
//!
//! Tests the pulse and cron systems using mock providers and temporary workspaces.

#[expect(
    clippy::tests_outside_test_module,
    reason = "integration tests live in tests/ directory, not inside #[cfg(test)] modules"
)]
mod proactivity_integration {
    use tempfile::tempdir;

    use residuum::bus::EventTrigger;
    use residuum::pulse::executor::build_pulse_execution;
    use residuum::pulse::scheduler::PulseScheduler;
    use residuum::pulse::types::{PulseDef, PulseTask};

    fn sample_pulse() -> PulseDef {
        PulseDef {
            name: "email_check".to_string(),
            enabled: true,
            schedule: "30m".to_string(),
            active_hours: None,
            agent: None,
            model_tier: None,
            include_identity: None,
            tasks: vec![PulseTask {
                name: "check_inbox".to_string(),
                prompt: "Check email.".to_string(),
            }],
        }
    }

    // ── build_pulse_execution tests ────────────────────────────────────────

    #[test]
    fn build_pulse_execution_no_agent_has_no_skill() {
        let pulse = sample_pulse();
        let spawn_event = build_pulse_execution(&pulse, None);
        assert_eq!(spawn_event.skill, None);
        assert_eq!(spawn_event.source_label, "pulse:email_check");
        assert!(matches!(spawn_event.source, EventTrigger::Pulse));
    }

    #[test]
    fn build_pulse_execution_prompt_contains_pulse_name_and_heartbeat_ok() {
        let pulse = sample_pulse();
        let spawn_event = build_pulse_execution(&pulse, None);
        assert!(
            spawn_event.prompt.contains("email_check"),
            "prompt should contain pulse name"
        );
        assert!(
            spawn_event.prompt.contains("check_inbox"),
            "prompt should contain task name"
        );
        assert!(
            spawn_event.prompt.contains("Check email"),
            "prompt should contain task prompt"
        );
        assert!(
            spawn_event.prompt.contains("HEARTBEAT_OK"),
            "prompt should contain HEARTBEAT_OK instruction"
        );
    }

    #[test]
    fn build_pulse_execution_empty_tasks_still_builds() {
        let pulse = PulseDef {
            name: "empty_test".to_string(),
            enabled: true,
            schedule: "1h".to_string(),
            active_hours: None,
            agent: None,
            model_tier: None,
            include_identity: None,
            tasks: vec![],
        };

        let spawn_event = build_pulse_execution(&pulse, None);
        assert_eq!(spawn_event.source_label, "pulse:empty_test");
        assert!(
            spawn_event.prompt.contains("HEARTBEAT_OK"),
            "should still have HEARTBEAT_OK instruction with no tasks"
        );
    }

    #[test]
    fn build_pulse_execution_agent_name_activates_skill() {
        let mut pulse = sample_pulse();
        pulse.agent = Some("memory-agent".to_string());
        let spawn_event = build_pulse_execution(&pulse, None);
        assert_eq!(
            spawn_event.skill.as_ref().map(AsRef::as_ref),
            Some("memory-agent")
        );
        assert_eq!(spawn_event.source_label, "pulse:email_check");
    }

    #[test]
    fn pulse_with_agent_main_fails_validation() {
        use residuum::pulse::types::validate_pulse;

        let mut pulse = sample_pulse();
        pulse.agent = Some("main".to_string());
        let err = validate_pulse(&pulse).expect_err("agent: main must be rejected");
        assert!(err.contains("email_check"));
        assert!(err.contains("agent: \"main\""));
    }

    #[test]
    fn pulse_with_include_identity_fails_validation() {
        use residuum::pulse::types::validate_pulse;

        let mut pulse = sample_pulse();
        pulse.include_identity = Some(true);
        let err = validate_pulse(&pulse).expect_err("include_identity must be rejected");
        assert!(err.contains("email_check"));
        assert!(err.contains("include_identity"));
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

    // ── Action store tests ────────────────────────────────────────────────────

    #[tokio::test]
    async fn action_store_round_trip() {
        use residuum::actions::store::ActionStore;
        use residuum::actions::types::ScheduledAction;

        let dir = tempdir().unwrap();
        let path = dir.path().join("scheduled_actions.json");

        let now = chrono::Utc::now();
        let action = ScheduledAction {
            id: "action-test0001".to_string(),
            name: "test action".to_string(),
            prompt: "Run a check.".to_string(),
            run_at: now + chrono::Duration::hours(1),
            agent: Some("memory-agent".to_string()),
            model_tier: None,
            created_at: now,
        };

        let (mut store, rejected_on_first_load, _) = ActionStore::load(&path).await.unwrap();
        assert!(rejected_on_first_load.is_empty());
        store.add(action);
        store.save().await.unwrap();

        let (reloaded, rejected_on_reload, _) = ActionStore::load(&path).await.unwrap();
        assert!(rejected_on_reload.is_empty());
        assert_eq!(
            reloaded.list().len(),
            1,
            "should have one action after reload"
        );

        let loaded = reloaded.list().first().unwrap();
        assert_eq!(
            loaded.id, "action-test0001",
            "action id should survive reload"
        );
        assert_eq!(loaded.name, "test action", "name should survive reload");
        assert_eq!(
            loaded.prompt, "Run a check.",
            "prompt should survive reload"
        );
        assert_eq!(
            loaded.agent.as_deref(),
            Some("memory-agent"),
            "agent should survive reload"
        );
    }

    #[tokio::test]
    async fn action_store_take_due() {
        use residuum::actions::store::ActionStore;
        use residuum::actions::types::ScheduledAction;

        let dir = tempdir().unwrap();
        let path = dir.path().join("scheduled_actions.json");

        let now = chrono::Utc::now();
        let past_action = ScheduledAction {
            id: "action-past".to_string(),
            name: "past".to_string(),
            prompt: "overdue".to_string(),
            run_at: now - chrono::Duration::minutes(5),
            agent: None,
            model_tier: None,
            created_at: now,
        };
        let future_action = ScheduledAction {
            id: "action-future".to_string(),
            name: "future".to_string(),
            prompt: "not yet".to_string(),
            run_at: now + chrono::Duration::hours(1),
            agent: None,
            model_tier: None,
            created_at: now,
        };

        let (mut store, _rejected, _moved_aside) = ActionStore::load(&path).await.unwrap();
        store.add(past_action);
        store.add(future_action);

        let due = store.take_due(now);
        assert_eq!(due.len(), 1, "only the past action should be due");
        assert_eq!(due.first().unwrap().id, "action-past");
        assert_eq!(store.list().len(), 1, "future action should remain");
    }

    #[tokio::test]
    async fn action_store_remove() {
        use residuum::actions::store::ActionStore;
        use residuum::actions::types::ScheduledAction;

        let dir = tempdir().unwrap();
        let path = dir.path().join("scheduled_actions.json");

        let now = chrono::Utc::now();
        let action = ScheduledAction {
            id: "action-cancel-me".to_string(),
            name: "cancel me".to_string(),
            prompt: "test".to_string(),
            run_at: now + chrono::Duration::hours(1),
            agent: None,
            model_tier: None,
            created_at: now,
        };

        let (mut store, _rejected, _moved_aside) = ActionStore::load(&path).await.unwrap();
        store.add(action);
        assert_eq!(store.list().len(), 1);

        assert!(store.remove("action-cancel-me"), "should find and remove");
        assert!(store.list().is_empty(), "store should be empty");
        assert!(
            !store.remove("action-cancel-me"),
            "should return false for missing"
        );
    }
}
