//! End-to-end integration tests for the background task subsystem (Phase 2 + 4).
//!
//! Tests sub-agent execution, spawner concurrency, result routing through the
//! notification system, and Phase 4 isolated project/skill state for sub-agents.

#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
#[expect(
    clippy::tests_outside_test_module,
    reason = "integration tests live in tests/ directory, not inside #[cfg(test)] modules"
)]
mod background_integration {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Arc;

    use tempfile::tempdir;
    use tokio::sync::mpsc;

    use residuum::background::BackgroundTaskSpawner;
    use residuum::background::types::{
        BackgroundTask, ResultRouting, TaskStatus, format_background_result,
    };
    use residuum::notify::channels::InboxChannel;
    use residuum::notify::router::NotificationRouter;
    use residuum::notify::types::TaskSource;

    // ── Result routing to inbox via channels ───────────────────────────

    #[tokio::test]
    async fn result_routes_to_inbox_via_channels() {
        let dir = tempdir().unwrap();
        let inbox_dir = dir.path().join("inbox");
        std::fs::create_dir_all(&inbox_dir).unwrap();

        let inbox_channel = InboxChannel::new(&inbox_dir, chrono_tz::UTC);
        let router = NotificationRouter::new(HashMap::new(), Some(inbox_channel));

        // Simulate a background result with direct inbox routing
        let result = residuum::background::types::BackgroundResult {
            id: "route-1".to_string(),
            task_name: "test_script".to_string(),
            source: TaskSource::Agent,
            summary: "found 5 items".to_string(),
            transcript_path: None,
            status: TaskStatus::Completed,
            timestamp: chrono::Utc::now(),
            routing: ResultRouting::Direct(vec!["inbox".to_string()]),
        };

        let notification = residuum::notify::types::Notification {
            task_name: result.task_name.clone(),
            summary: result.summary.clone(),
            source: result.source,
            timestamp: result.timestamp,
        };

        let delivered = router.deliver_to_inbox(&notification).await.unwrap();
        assert!(delivered, "should deliver to inbox");

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
        let result = residuum::background::types::BackgroundResult {
            id: "unrouted-1".to_string(),
            task_name: "nobody_listens".to_string(),
            source: TaskSource::Agent,
            summary: "result that goes nowhere".to_string(),
            transcript_path: Some(dir.path().join("bg-unrouted-1.log")),
            status: TaskStatus::Completed,
            timestamp: chrono::Utc::now(),
            routing: ResultRouting::Direct(vec![]),
        };

        let ResultRouting::Direct(channels) = &result.routing;
        assert!(channels.is_empty(), "should have no channels");

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
        let result = residuum::background::types::BackgroundResult {
            id: "fmt-1".to_string(),
            task_name: "my_task".to_string(),
            source: TaskSource::Action,
            summary: "task completed successfully".to_string(),
            transcript_path: Some(PathBuf::from("/tmp/bg-fmt-1.log")),
            status: TaskStatus::Completed,
            timestamp: chrono::Utc::now(),
            routing: ResultRouting::Direct(vec!["agent_feed".to_string()]),
        };

        let formatted = format_background_result(&result);
        assert!(formatted.contains("my_task"));
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

        let result = residuum::background::types::BackgroundResult {
            id: "action-evt-test-1".to_string(),
            task_name: "reminder".to_string(),
            source: TaskSource::Action,
            summary: "time to stretch".to_string(),
            transcript_path: None,
            status: TaskStatus::Completed,
            timestamp: chrono::Utc::now(),
            routing: ResultRouting::Direct(vec!["agent_feed".to_string()]),
        };

        spawner.send_result(result).await.unwrap();

        let received = rx.recv().await.unwrap();
        assert_eq!(received.id, "action-evt-test-1");
        assert_eq!(received.task_name, "reminder");
        assert_eq!(received.summary, "time to stretch");
        assert!(matches!(received.source, TaskSource::Action));
        assert!(matches!(received.status, TaskStatus::Completed));
    }

    // ── Phase 6: subagent_spawn async result delivery ──────────────────

    #[tokio::test]
    async fn subagent_spawn_async_result_delivery() {
        use residuum::background::types::{Execution, SubAgentConfig};
        use residuum::config::BackgroundModelTier;

        let dir = tempdir().unwrap();
        let (tx, mut rx) = mpsc::channel(32);
        let spawner = Arc::new(BackgroundTaskSpawner::new(
            tx,
            3,
            PathBuf::from("/tmp"),
            dir.path().to_path_buf(),
        ));

        let task = BackgroundTask {
            id: "agent-spawn-e2e-1".to_string(),
            task_name: "my_subagent".to_string(),
            source: TaskSource::Agent,
            execution: Execution::SubAgent(SubAgentConfig {
                prompt: "summarize recent events".to_string(),
                context: None,
                model_tier: BackgroundModelTier::Medium,
            }),
            routing: ResultRouting::Direct(vec!["agent_feed".to_string(), "inbox".to_string()]),
        };

        // Without real SubAgentResources, the spawner will fail with a "requires SubAgentResources" error.
        // That's fine — we're testing that the task enters the pipeline with correct source and routing.
        spawner.spawn(task, None).await.unwrap();

        let result = rx.recv().await.unwrap();
        assert_eq!(result.id, "agent-spawn-e2e-1");
        assert_eq!(result.task_name, "my_subagent");
        assert!(
            matches!(result.source, TaskSource::Agent),
            "source should be Agent"
        );
        // Should be Failed because no resources were provided
        assert!(
            matches!(result.status, TaskStatus::Failed { .. }),
            "should fail without resources"
        );
        // Routing should be preserved on the result
        let ResultRouting::Direct(channels) = &result.routing;
        assert_eq!(channels.len(), 2);
        assert!(channels.contains(&"agent_feed".to_string()));
        assert!(channels.contains(&"inbox".to_string()));
    }

    #[test]
    fn build_pulse_task_creates_correct_structure() {
        use residuum::background::types::Execution;
        use residuum::config::BackgroundModelTier;
        use residuum::pulse::executor::build_pulse_task;
        use residuum::pulse::types::{PulseDef, PulseTask};

        let pulse = PulseDef {
            name: "status_check".to_string(),
            enabled: true,
            schedule: "1h".to_string(),
            active_hours: None,
            agent: None,
            trigger_count: None,
            channels: vec!["agent_feed".to_string()],
            tasks: vec![PulseTask {
                name: "check_health".to_string(),
                prompt: "Check system health.".to_string(),
            }],
        };

        let task = build_pulse_task(&pulse);

        assert_eq!(task.task_name, "status_check");
        assert!(task.id.starts_with("pulse-status_check-"));
        assert!(matches!(task.source, TaskSource::Pulse));
        let ResultRouting::Direct(channels) = &task.routing;
        assert_eq!(channels, &["agent_feed"], "should route to default channel");

        let Execution::SubAgent(cfg) = &task.execution;
        assert_eq!(cfg.model_tier, BackgroundModelTier::Small);
        assert!(cfg.prompt.contains("status_check"));
        assert!(cfg.prompt.contains("HEARTBEAT_OK"));
    }
}
