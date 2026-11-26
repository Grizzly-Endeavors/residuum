//! End-to-end integration tests for agent sessions.
//!
//! Tests result routing through the notification system, the session
//! registry's public discovery surface, and pulse execution's spawn-request
//! structure. Session lifecycle mechanics (fork → running → idle →
//! completed, transcript persistence) are covered where they're exercised —
//! `crates/background/src/runtime.rs` and `crates/background/src/store.rs` — since driving
//! them from here would mean re-exposing crate-internal construction just
//! for the test.
//!
//! covers: residuum-background, residuum-notify, residuum-proactivity

#![expect(
    clippy::tests_outside_test_module,
    reason = "integration tests live in tests/ directory, not inside #[cfg(test)] modules"
)]

use std::sync::Arc;
use std::time::Duration;

use tempfile::tempdir;

use residuum::background::registry::SessionRegistry;
use residuum::background::testing::{
    make_sequenced_resources, next_idle, sample_conversation_request, test_runtime_held_idle,
    test_spawn_context,
};
use residuum::background::{ConversationCloseFlush, SessionCloseFlush};
use residuum::bus::testing::next_event;
use residuum::bus::{
    AgentResultStatus, EventTrigger, NotificationEvent, SessionAddress, SessionEvent,
    SessionEventKind, spawn_broker, topics,
};
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

    residuum_core::testing::wait::until("an inbox item to be created", || {
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
    let unknown = SessionAddress::from("spawned-ghost-0000");
    assert!(!registry.stop(&unknown));
    assert!(!registry.cancel_current_turn(&unknown));
}

#[tokio::test]
async fn flush_and_close_conversation_closes_idle_session_with_unseen_tail() {
    let (runtime, _results, bus_handle) = test_runtime_held_idle().await;
    let runtime = Arc::new(runtime);
    let mut events: residuum::bus::Subscriber<SessionEvent> =
        bus_handle.subscribe(topics::Sessions).await.unwrap();

    let address = SessionAddress::from("external-flush-it-0001");
    let resources = make_sequenced_resources(vec!["initial reply"]);
    let ctx = test_spawn_context(
        Arc::clone(&runtime),
        &bus_handle,
        resources.layout.clone(),
        Arc::clone(&resources.observer),
        Arc::clone(&resources.merge_writer),
    );

    runtime.spawn(
        sample_conversation_request(address.as_ref()),
        Some(resources),
    );
    next_idle(&mut events, &address, "session to finish initial turn").await;

    ctx.flush_and_close_conversation(ConversationCloseFlush {
        address: address.clone(),
        source_label: "conversation:test".to_string(),
        conversation_target: None,
        flush: SessionCloseFlush {
            user_content: Some("unseen closing transcript".to_string()),
            assistant_note: "conversation closed".to_string(),
        },
    });

    loop {
        let ev = next_event("completed event after close flush", &mut events).await;
        if ev.address == address
            && let SessionEventKind::Completed {
                status,
                memory_pending,
            } = ev.kind
        {
            assert!(
                matches!(status, AgentResultStatus::Completed),
                "close flush must complete the run, got {status:?}"
            );
            assert!(memory_pending, "close flush must trigger a memory merge");
            break;
        }
    }
    runtime.shutdown(Duration::from_secs(5)).await;
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
        schedule: Some("1h".to_string()),
        days: None,
        active_hours: None,
        agent: None,
        model_tier: None,
        context_from: None,
        include_identity: None,
        tasks: vec![PulseTask {
            name: "check_health".to_string(),
            prompt: "Check system health.".to_string(),
            ..PulseTask::default()
        }],
    };

    let spawn_event = build_pulse_execution(&pulse, None, None, &[]);
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
