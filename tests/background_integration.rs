//! End-to-end integration tests for agent sessions.
//!
//! Tests result routing through the notification system, the session
//! registry's public discovery surface, and pulse execution's spawn-request
//! structure. Session lifecycle mechanics (fork → running → idle →
//! completed, transcript persistence) are covered where they're exercised —
//! `src/background/runtime.rs` and `src/background/store.rs` — since driving
//! them from here would mean re-exposing crate-internal construction just
//! for the test.

#[path = "support/until.rs"]
mod until;

#[expect(
    clippy::tests_outside_test_module,
    reason = "integration tests live in tests/ directory, not inside #[cfg(test)] modules"
)]
mod background_integration {
    use tempfile::tempdir;

    use residuum::background::registry::SessionRegistry;
    use residuum::bus::{EventTrigger, NotificationEvent, SessionAddress, spawn_broker, topics};
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
        let subscriber = handle.subscribe(topics::Inbox).await.unwrap();
        let inbox_channel = InboxChannel::new(&inbox_dir, chrono_tz::UTC);

        let loop_task = tokio::spawn(run_notify_subscriber(subscriber, Box::new(inbox_channel)));

        let notification = NotificationEvent {
            title: "test_script".to_string(),
            content: "found 5 items".to_string(),
            source: EventTrigger::Agent,
            urgent: false,
            timestamp: chrono::NaiveDate::from_ymd_opt(2026, 3, 14)
                .unwrap()
                .and_hms_opt(12, 0, 0)
                .unwrap(),
        };

        publisher
            .publish(topics::Inbox, notification)
            .await
            .unwrap();

        crate::until::until("an inbox item to be created", || {
            let count = std::fs::read_dir(&inbox_dir)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
                .count();
            std::future::ready((count >= 1).then_some(()))
        })
        .await;

        // Abort the subscriber loop
        loop_task.abort();
    }

    // ── Session registry discovery surface ──────────────────────────────

    #[tokio::test]
    async fn stopping_an_unknown_address_reports_not_found() {
        let registry = SessionRegistry::new();
        assert!(!registry.stop(&SessionAddress::from("spawned-ghost-0000")));
    }

    #[tokio::test]
    async fn a_freshly_constructed_registry_has_no_live_sessions() {
        let registry = SessionRegistry::new();
        assert!(registry.list_live().is_empty());
        assert!(registry.subagent_snapshot().is_empty());
    }

    // ── Pulse execution builds correct structure ──────────────────────

    #[test]
    fn build_pulse_execution_creates_correct_structure() {
        use residuum::pulse::executor::build_pulse_execution;
        use residuum::pulse::types::{PulseDef, PulseTask};

        let pulse = PulseDef {
            name: "status_check".to_string(),
            enabled: true,
            schedule: "1h".to_string(),
            active_hours: None,
            agent: None,
            model_tier: None,
            context_from: None,
            include_identity: None,
            tasks: vec![PulseTask {
                name: "check_health".to_string(),
                prompt: "Check system health.".to_string(),
            }],
        };

        let spawn_event = build_pulse_execution(&pulse, None, None);
        assert_eq!(spawn_event.skill, None);
        assert_eq!(spawn_event.source_label, "pulse:status_check");
        assert!(spawn_event.prompt.contains("status_check"));
        assert!(spawn_event.prompt.contains("HEARTBEAT_OK"));
        assert!(
            spawn_event
                .address
                .as_ref()
                .starts_with("scheduled-status-check-")
        );
    }
}
