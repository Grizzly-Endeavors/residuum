//! End-to-end integration tests for the background task subsystem (Phase 2).
//!
//! Tests script execution, sub-agent execution, spawner concurrency, and
//! result routing through the notification system.

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
        let spawner = BackgroundTaskSpawner::new(
            tx,
            3,
            PathBuf::from("/tmp"),
            dir.path().to_path_buf(),
            chrono_tz::UTC,
        );

        let task = make_script_task("e2e-1", "echo", &["hello world"]);
        spawner.spawn(task, None).unwrap();

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
        let spawner = BackgroundTaskSpawner::new(
            tx,
            3,
            PathBuf::from("/tmp"),
            dir.path().to_path_buf(),
            chrono_tz::UTC,
        );

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

        spawner.spawn(task, None).unwrap();

        let result = rx.recv().await.unwrap();
        assert_eq!(result.id, "fail-1");
        // `false` returns exit code 1, which is captured as Completed with exit code info
        assert!(
            matches!(result.status, TaskStatus::Completed),
            "non-zero exit is captured as completed with exit info"
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
        let spawner = BackgroundTaskSpawner::new(
            tx,
            max,
            PathBuf::from("/tmp"),
            dir.path().to_path_buf(),
            chrono_tz::UTC,
        );

        // Spawn max + 1 tasks
        for i in 0..=max {
            let task = make_script_task(&format!("conc-{i}"), "echo", &[&format!("task-{i}")]);
            spawner.spawn(task, None).unwrap();
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
            chrono_tz::UTC,
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

        spawner.spawn(task, None).unwrap();

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
            chrono_tz::UTC,
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

        spawner.spawn(task, None).unwrap();
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
            chrono_tz::UTC,
        ));

        let not_found = spawner.cancel("does-not-exist").await;
        assert!(!not_found, "cancel should return false for unknown task");
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
}
