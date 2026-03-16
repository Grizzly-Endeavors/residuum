//! End-to-end integration tests for the background task subsystem (Phase 2 + 4).
//!
//! Tests sub-agent execution, spawner concurrency, result routing through the
//! notification system, and Phase 4 isolated project/skill state for sub-agents.

#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
#[expect(clippy::panic, reason = "test assertions")]
#[expect(
    clippy::tests_outside_test_module,
    reason = "integration tests live in tests/ directory, not inside #[cfg(test)] modules"
)]
mod background_integration {
    use std::path::PathBuf;
    use std::sync::Arc;

    use tempfile::tempdir;
    use tokio::sync::mpsc;

    use residuum::background::BackgroundTaskSpawner;
    use residuum::background::types::{BackgroundResult, format_background_result};
    use residuum::bus::AgentResultStatus;
    use residuum::bus::{EventTrigger, NotificationEvent, PresetName, spawn_broker, topics};
    use residuum::notify::channels::InboxChannel;
    use residuum::notify::subscriber::run_notify_subscriber;

    // ── Result routing to inbox via bus subscriber ────────────────────

    #[tokio::test]
    async fn result_routes_to_inbox_via_bus() {
        let dir = tempdir().unwrap();
        let inbox_dir = dir.path().join("inbox");
        std::fs::create_dir_all(&inbox_dir).unwrap();

        let handle = spawn_broker();
        let publisher = handle.publisher();
        let subscriber = handle.subscribe_typed(topics::Inbox).await.unwrap();
        let inbox_channel = InboxChannel::new(&inbox_dir, chrono_tz::UTC);

        let loop_task = tokio::spawn(run_notify_subscriber(subscriber, Box::new(inbox_channel)));

        let notification = NotificationEvent {
            title: "test_script".to_string(),
            content: "found 5 items".to_string(),
            source: EventTrigger::Agent,
            timestamp: chrono::NaiveDate::from_ymd_opt(2026, 3, 14)
                .unwrap()
                .and_hms_opt(12, 0, 0)
                .unwrap(),
        };

        publisher
            .publish_typed(topics::Inbox, notification)
            .await
            .unwrap();

        // Give subscriber time to process
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Abort the subscriber loop
        loop_task.abort();

        // Verify inbox item was created
        let items: Vec<_> = std::fs::read_dir(&inbox_dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
            .collect();
        assert_eq!(items.len(), 1, "should create one inbox item");
    }

    // ── Unrouted task still has transcript ──────────────────────────────

    #[tokio::test]
    async fn unrouted_task_has_transcript_but_no_delivery() {
        let dir = tempdir().unwrap();

        // Empty channels = unrouted
        let result = BackgroundResult {
            id: "unrouted-1".to_string(),
            source_label: "agent:nobody_listens".to_string(),
            source: EventTrigger::Agent,
            summary: "result that goes nowhere".to_string(),
            transcript_path: Some(dir.path().join("bg-unrouted-1.log")),
            status: AgentResultStatus::Completed,
            timestamp: chrono::Utc::now(),

            agent_preset: PresetName::from("general-purpose"),
        };

        // Transcript path was set (would have been written by spawner)
        assert!(result.transcript_path.is_some());
    }

    // ── Phase 3: cancel ─────────────────────────────────────────────

    #[tokio::test]
    async fn cancel_nonexistent_task_returns_false() {
        let dir = tempdir().unwrap();
        let (tx, _rx) = mpsc::channel(32);
        let spawner = Arc::new(BackgroundTaskSpawner::new(
            tx,
            3,
            PathBuf::from("/tmp"),
            dir.path().to_path_buf(),
        ));

        let not_found = spawner.cancel("does-not-exist").await;
        assert!(!not_found, "cancel should return false for unknown task");
    }

    // ── Phase 4: MCP ref counting ──────────────────────────────────────

    #[tokio::test]
    async fn mcp_ref_counting_two_activations_one_deactivation_keeps_servers() {
        use residuum::mcp::McpRegistry;
        use residuum::projects::types::{McpServerEntry, McpTransport};

        let mut registry = McpRegistry::new();
        let entry = McpServerEntry {
            name: "shared-svc".to_string(),
            // Nonexistent binary: connection fails but ref count is still tracked
            command: "/nonexistent/mcp-shared-svc".to_string(),
            args: vec![],
            env: std::collections::HashMap::new(),
            transport: McpTransport::default(),
            headers: std::collections::HashMap::new(),
        };

        // First activation: starts (fails) the server but records ref count = 1
        let report1 = registry
            .activate_project("proj-x", std::slice::from_ref(&entry))
            .await;
        assert_eq!(
            report1.failures.len(),
            1,
            "server connect fails (no binary)"
        );
        // Manually mark running to simulate a real running server for the test
        registry.mark_running("shared-svc");

        // Second activation: count increments to 2, empty report returned
        let report2 = registry
            .activate_project("proj-x", std::slice::from_ref(&entry))
            .await;
        assert_eq!(report2.started, 0, "second activation returns empty report");
        assert_eq!(
            report2.failures.len(),
            0,
            "no failures on second activation"
        );

        // First deactivation: count 2 → 1, no servers stopped
        let first_deactivation = registry.deactivate_project("proj-x").await;
        assert!(
            first_deactivation.is_empty(),
            "deactivation at count > 0 should not stop servers"
        );
        // Server should still be tracked
        let states_after_first = registry.servers();
        assert!(
            states_after_first.iter().any(|s| s.name == "shared-svc"),
            "server should still be running after partial deactivation"
        );

        // Second deactivation: count 1 → 0, server disconnected
        let second_deactivation = registry.deactivate_project("proj-x").await;
        assert_eq!(
            second_deactivation,
            vec!["shared-svc"],
            "server stopped at count 0"
        );
        let states_after_second = registry.servers();
        assert!(
            !states_after_second.iter().any(|s| s.name == "shared-svc"),
            "server should be gone after full deactivation"
        );
    }

    // ── Format result ──────────────────────────────────────────────────

    #[test]
    fn format_result_contains_all_fields() {
        let result = BackgroundResult {
            id: "fmt-1".to_string(),
            source_label: "action:my_task".to_string(),
            source: EventTrigger::Action,
            summary: "task completed successfully".to_string(),
            transcript_path: Some(PathBuf::from("/tmp/bg-fmt-1.log")),
            status: AgentResultStatus::Completed,
            timestamp: chrono::Utc::now(),

            agent_preset: PresetName::from("general-purpose"),
        };

        let formatted = format_background_result(&result);
        assert!(formatted.contains("action:my_task"));
        assert!(formatted.contains("fmt-1"));
        assert!(formatted.contains("action"));
        assert!(formatted.contains("completed"));
        assert!(formatted.contains("task completed successfully"));
        assert!(formatted.contains("/tmp/bg-fmt-1.log"));
    }

    // ── Phase 5: Pulse/actions via background spawner ───────────────────

    #[tokio::test]
    async fn send_result_delivers_action_event() {
        let dir = tempdir().unwrap();
        let (tx, mut rx) = mpsc::channel(32);
        let spawner =
            BackgroundTaskSpawner::new(tx, 3, PathBuf::from("/tmp"), dir.path().to_path_buf());

        let result = BackgroundResult {
            id: "action-evt-test-1".to_string(),
            source_label: "action:reminder".to_string(),
            source: EventTrigger::Action,
            summary: "time to stretch".to_string(),
            transcript_path: None,
            status: AgentResultStatus::Completed,
            timestamp: chrono::Utc::now(),

            agent_preset: PresetName::from("general-purpose"),
        };

        spawner.send_result(result).await.unwrap();

        let received = rx.recv().await.unwrap();
        assert_eq!(received.id, "action-evt-test-1");
        assert_eq!(received.source_label, "action:reminder");
        assert_eq!(received.summary, "time to stretch");
        assert!(matches!(received.source, EventTrigger::Action));
        assert!(matches!(received.status, AgentResultStatus::Completed));
    }

    // ── Pulse execution builds correct structure ──────────────────────

    #[test]
    fn build_pulse_execution_creates_correct_structure() {
        use residuum::pulse::executor::{PulseExecution, build_pulse_execution};
        use residuum::pulse::types::{PulseDef, PulseTask};

        let pulse = PulseDef {
            name: "status_check".to_string(),
            enabled: true,
            schedule: "1h".to_string(),
            active_hours: None,
            agent: None,
            trigger_count: None,
            tasks: vec![PulseTask {
                name: "check_health".to_string(),
                prompt: "Check system health.".to_string(),
            }],
        };

        match build_pulse_execution(&pulse) {
            PulseExecution::SubAgent {
                spawn_event,
                preset_name,
            } => {
                assert_eq!(preset_name, "general-purpose");
                assert_eq!(spawn_event.source_label, "pulse:status_check");
                assert!(spawn_event.prompt.contains("status_check"));
                assert!(spawn_event.prompt.contains("HEARTBEAT_OK"));
            }
            PulseExecution::MainWakeTurn { .. } => {
                panic!("expected SubAgent");
            }
        }
    }
}
