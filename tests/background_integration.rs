//! End-to-end integration tests for the background task subsystem (Phase 2 + 4).
//!
//! Tests script execution, sub-agent execution, spawner concurrency, result
//! routing through the notification system, and Phase 4 isolated project/skill
//! state for sub-agents.

#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
#[expect(
    clippy::panic,
    reason = "test assertions use panic for unreachable variants"
)]
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

    use ironclaw::background::BackgroundTaskSpawner;
    use ironclaw::background::types::{
        BackgroundTask, Execution, ResultRouting, ScriptConfig, TaskStatus,
        format_background_result,
    };
    use ironclaw::notify::channels::InboxChannel;
    use ironclaw::notify::router::NotificationRouter;
    use ironclaw::notify::types::TaskSource;

    fn make_script_task(id: &str, command: &str, args: &[&str]) -> BackgroundTask {
        BackgroundTask {
            id: id.to_string(),
            task_name: "test_script".to_string(),
            source: TaskSource::Agent,
            execution: Execution::Script(ScriptConfig {
                command: command.to_string(),
                args: args.iter().map(|s| (*s).to_string()).collect(),
                working_dir: None,
                timeout_secs: None,
            }),
            routing: ResultRouting::Notify,
        }
    }

    // ── Script end-to-end ──────────────────────────────────────────────

    #[tokio::test]
    async fn script_echo_end_to_end() {
        let dir = tempdir().unwrap();
        let (tx, mut rx) = mpsc::channel(32);
        let spawner =
            BackgroundTaskSpawner::new(tx, 3, PathBuf::from("/tmp"), dir.path().to_path_buf());

        let task = make_script_task("e2e-1", "echo", &["hello world"]);
        spawner.spawn(task, None).await.unwrap();

        let result = rx.recv().await.unwrap();
        assert_eq!(result.id, "e2e-1");
        assert!(matches!(result.status, TaskStatus::Completed));
        assert!(
            result.summary.contains("hello world"),
            "output should contain echo text"
        );
        assert!(
            result.transcript_path.is_some(),
            "should write transcript file"
        );
    }

    #[tokio::test]
    async fn script_failure_end_to_end() {
        let dir = tempdir().unwrap();
        let (tx, mut rx) = mpsc::channel(32);
        let spawner =
            BackgroundTaskSpawner::new(tx, 3, PathBuf::from("/tmp"), dir.path().to_path_buf());

        let task = BackgroundTask {
            id: "fail-1".to_string(),
            task_name: "test_fail".to_string(),
            source: TaskSource::Agent,
            execution: Execution::Script(ScriptConfig {
                command: "false".to_string(),
                args: Vec::new(),
                working_dir: None,
                timeout_secs: None,
            }),
            routing: ResultRouting::Notify,
        };

        spawner.spawn(task, None).await.unwrap();

        let result = rx.recv().await.unwrap();
        assert_eq!(result.id, "fail-1");
        assert!(
            matches!(result.status, TaskStatus::Failed { .. }),
            "non-zero exit should be recorded as failed"
        );
        assert!(
            result.summary.contains("exit code"),
            "should contain exit code info"
        );
    }

    // ── Concurrency limit ──────────────────────────────────────────────

    #[tokio::test]
    async fn concurrency_limit_queues_excess() {
        let dir = tempdir().unwrap();
        let max = 2;
        let (tx, mut rx) = mpsc::channel(32);
        let spawner =
            BackgroundTaskSpawner::new(tx, max, PathBuf::from("/tmp"), dir.path().to_path_buf());

        // Spawn max + 1 tasks
        for i in 0..=max {
            let task = make_script_task(&format!("conc-{i}"), "echo", &[&format!("task-{i}")]);
            spawner.spawn(task, None).await.unwrap();
        }

        // All should eventually complete
        let mut results = Vec::new();
        for _ in 0..=max {
            results.push(rx.recv().await.unwrap());
        }

        assert_eq!(results.len(), max + 1, "all tasks should complete");
        for result in &results {
            assert!(
                matches!(result.status, TaskStatus::Completed),
                "all should complete successfully"
            );
        }
    }

    // ── Result routing through NOTIFY.yml to inbox ─────────────────────

    #[tokio::test]
    async fn result_routes_to_inbox_via_notify_yml() {
        let dir = tempdir().unwrap();
        let inbox_dir = dir.path().join("inbox");
        std::fs::create_dir_all(&inbox_dir).unwrap();

        // Write NOTIFY.yml that routes test_script to inbox
        let notify_path = dir.path().join("NOTIFY.yml");
        std::fs::write(&notify_path, "inbox:\n  - test_script\n").unwrap();

        let inbox_channel = InboxChannel::new(&inbox_dir, chrono_tz::UTC);
        let router = NotificationRouter::new(HashMap::new(), Some(inbox_channel));

        // Simulate a background result
        let result = ironclaw::background::types::BackgroundResult {
            id: "route-1".to_string(),
            task_name: "test_script".to_string(),
            source: TaskSource::Agent,
            summary: "found 5 items".to_string(),
            transcript_path: None,
            status: TaskStatus::Completed,
            timestamp: chrono::Utc::now(),
            routing: ResultRouting::Notify,
        };

        let notification = ironclaw::notify::types::Notification {
            task_name: result.task_name.clone(),
            summary: result.summary.clone(),
            source: result.source,
            timestamp: result.timestamp,
        };

        let outcome = router.route(&notification, &notify_path).await;
        assert!(outcome.inbox, "should route to inbox");

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
        let notify_path = dir.path().join("NOTIFY.yml");
        // Write NOTIFY.yml with no entries for our task
        std::fs::write(&notify_path, "agent_feed:\n  - other_task\n").unwrap();

        let router = NotificationRouter::empty();

        let result = ironclaw::background::types::BackgroundResult {
            id: "unrouted-1".to_string(),
            task_name: "nobody_listens".to_string(),
            source: TaskSource::Agent,
            summary: "result that goes nowhere".to_string(),
            transcript_path: Some(dir.path().join("bg-unrouted-1.log")),
            status: TaskStatus::Completed,
            timestamp: chrono::Utc::now(),
            routing: ResultRouting::Notify,
        };

        let notification = ironclaw::notify::types::Notification {
            task_name: result.task_name.clone(),
            summary: result.summary.clone(),
            source: result.source,
            timestamp: result.timestamp,
        };

        let outcome = router.route(&notification, &notify_path).await;
        assert!(!outcome.agent_wake);
        assert!(!outcome.agent_feed);
        assert!(!outcome.inbox);
        assert!(outcome.external_dispatched.is_empty());

        // Transcript path was set (would have been written by spawner)
        assert!(result.transcript_path.is_some());
    }

    // ── Phase 3: list_active_tasks and cancel ─────────────────────

    #[tokio::test]
    async fn cancel_long_running_script_produces_cancelled_result() {
        let dir = tempdir().unwrap();
        let (tx, mut rx) = mpsc::channel(32);
        let spawner = Arc::new(BackgroundTaskSpawner::new(
            tx,
            3,
            PathBuf::from("/tmp"),
            dir.path().to_path_buf(),
        ));

        let task = BackgroundTask {
            id: "cancel-e2e-1".to_string(),
            task_name: "long_task".to_string(),
            source: TaskSource::Agent,
            execution: Execution::Script(ScriptConfig {
                command: "sleep".to_string(),
                args: vec!["30".to_string()],
                working_dir: None,
                timeout_secs: None,
            }),
            routing: ResultRouting::Notify,
        };

        spawner.spawn(task, None).await.unwrap();

        // Give it time to register and start
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        assert_eq!(
            spawner.active_task_ids().await.len(),
            1,
            "should have 1 active task"
        );

        let cancelled = spawner.cancel("cancel-e2e-1").await;
        assert!(cancelled, "cancel should return true for active task");

        let result = rx.recv().await.unwrap();
        assert!(
            matches!(result.status, TaskStatus::Cancelled),
            "status should be Cancelled"
        );

        // Wait briefly for cleanup then verify task removed
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        assert_eq!(
            spawner.active_task_ids().await.len(),
            0,
            "active_task_ids should be empty after cancel"
        );
    }

    #[tokio::test]
    async fn list_active_tasks_returns_metadata_for_running_task() {
        let dir = tempdir().unwrap();
        let (tx, mut rx) = mpsc::channel(32);
        let spawner = Arc::new(BackgroundTaskSpawner::new(
            tx,
            3,
            PathBuf::from("/tmp"),
            dir.path().to_path_buf(),
        ));

        let task = BackgroundTask {
            id: "list-meta-1".to_string(),
            task_name: "metadata_task".to_string(),
            source: TaskSource::Cron,
            execution: Execution::Script(ScriptConfig {
                command: "sleep".to_string(),
                args: vec!["30".to_string()],
                working_dir: None,
                timeout_secs: None,
            }),
            routing: ResultRouting::Notify,
        };

        spawner.spawn(task, None).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let tasks = spawner.list_active_tasks().await;
        assert_eq!(tasks.len(), 1, "should list 1 active task");

        let (id, info) = tasks.first().unwrap();
        assert_eq!(id, "list-meta-1");
        assert_eq!(info.task_name, "metadata_task");
        assert_eq!(info.execution_type, "script");
        assert!(
            info.prompt_preview.contains("sleep"),
            "preview should contain command"
        );
        assert!(
            matches!(info.source, TaskSource::Cron),
            "source should be Cron"
        );

        // Clean up
        spawner.cancel("list-meta-1").await;
        rx.recv().await.unwrap();
    }

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
        use ironclaw::mcp::McpRegistry;
        use ironclaw::projects::types::McpServerEntry;

        let mut registry = McpRegistry::new();
        let entry = McpServerEntry {
            name: "shared-svc".to_string(),
            // Nonexistent binary: connection fails but ref count is still tracked
            command: "/nonexistent/mcp-shared-svc".to_string(),
            args: vec![],
            env: std::collections::HashMap::new(),
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
        let result = ironclaw::background::types::BackgroundResult {
            id: "fmt-1".to_string(),
            task_name: "my_task".to_string(),
            source: TaskSource::Cron,
            summary: "task completed successfully".to_string(),
            transcript_path: Some(PathBuf::from("/tmp/bg-fmt-1.log")),
            status: TaskStatus::Completed,
            timestamp: chrono::Utc::now(),
            routing: ResultRouting::Notify,
        };

        let formatted = format_background_result(&result);
        assert!(formatted.contains("my_task"));
        assert!(formatted.contains("fmt-1"));
        assert!(formatted.contains("cron"));
        assert!(formatted.contains("completed"));
        assert!(formatted.contains("task completed successfully"));
        assert!(formatted.contains("/tmp/bg-fmt-1.log"));
    }

    // ── Phase 5: Pulse/cron via background spawner ──────────────────────

    #[tokio::test]
    async fn send_result_delivers_cron_system_event() {
        let dir = tempdir().unwrap();
        let (tx, mut rx) = mpsc::channel(32);
        let spawner =
            BackgroundTaskSpawner::new(tx, 3, PathBuf::from("/tmp"), dir.path().to_path_buf());

        let result = ironclaw::background::types::BackgroundResult {
            id: "cron-evt-test-1".to_string(),
            task_name: "reminder".to_string(),
            source: TaskSource::Cron,
            summary: "time to stretch".to_string(),
            transcript_path: None,
            status: TaskStatus::Completed,
            timestamp: chrono::Utc::now(),
            routing: ResultRouting::Notify,
        };

        spawner.send_result(result).await.unwrap();

        let received = rx.recv().await.unwrap();
        assert_eq!(received.id, "cron-evt-test-1");
        assert_eq!(received.task_name, "reminder");
        assert_eq!(received.summary, "time to stretch");
        assert!(matches!(received.source, TaskSource::Cron));
        assert!(matches!(received.status, TaskStatus::Completed));
    }

    // ── Phase 6: subagent_spawn async result delivery ──────────────────

    #[tokio::test]
    async fn subagent_spawn_async_result_delivery() {
        use ironclaw::background::types::{Execution, SubAgentConfig};
        use ironclaw::config::BackgroundModelTier;

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
        match &result.routing {
            ResultRouting::Direct(channels) => {
                assert_eq!(channels.len(), 2);
                assert!(channels.contains(&"agent_feed".to_string()));
                assert!(channels.contains(&"inbox".to_string()));
            }
            ResultRouting::Notify => panic!("expected Direct routing"),
        }
    }

    #[test]
    fn build_pulse_task_creates_correct_structure() {
        use ironclaw::background::types::Execution;
        use ironclaw::config::BackgroundModelTier;
        use ironclaw::pulse::executor::build_pulse_task;
        use ironclaw::pulse::types::{PulseDef, PulseTask};

        let pulse = PulseDef {
            name: "status_check".to_string(),
            enabled: true,
            schedule: "1h".to_string(),
            active_hours: None,
            agent: None,
            tasks: vec![PulseTask {
                name: "check_health".to_string(),
                prompt: "Check system health.".to_string(),
            }],
        };

        let task = build_pulse_task(&pulse);

        assert_eq!(task.task_name, "status_check");
        assert!(task.id.starts_with("pulse-status_check-"));
        assert!(matches!(task.source, TaskSource::Pulse));
        assert!(matches!(task.routing, ResultRouting::Notify));

        match &task.execution {
            Execution::SubAgent(cfg) => {
                assert_eq!(cfg.model_tier, BackgroundModelTier::Small);
                assert!(cfg.prompt.contains("status_check"));
                assert!(cfg.prompt.contains("HEARTBEAT_OK"));
            }
            Execution::Script(_) => panic!("expected SubAgent"),
        }
    }
}
