//! A conversation session answers its chat through `send_message` alone: a
//! message it sends to its chat posts there, and its final text stays in the
//! session.
//!
//! covers: residuum-background, residuum-tools
#![expect(
    clippy::tests_outside_test_module,
    reason = "integration tests live in tests/ directory, not inside #[cfg(test)] modules"
)]

use std::sync::Arc;

use residuum::background::testing::{
    make_sequenced_resources, next_idle, sample_conversation_request, test_runtime_held_idle,
};
use residuum::bus::testing::{bus_barrier, next_event};
use residuum::bus::{SessionAddress, topics};

#[tokio::test]
async fn a_conversation_session_s_send_message_to_its_chat_posts() {
    let log = residuum_core::testing::log::EventLog::default();
    let _capture = log.capture();
    let (runtime, _results, bus_handle) = test_runtime_held_idle().await;
    let mut session_events = bus_handle.subscribe(topics::Sessions).await.unwrap();
    let mut chat: residuum_bus::Subscriber<residuum_bus::ResponseEvent> = bus_handle
        .subscribe(topics::Endpoint(residuum_bus::EndpointName::from(
            "discord",
        )))
        .await
        .unwrap();
    let mut final_text: residuum_bus::Subscriber<residuum_bus::SessionResponseEvent> = bus_handle
        .subscribe(topics::Endpoint(residuum_bus::EndpointName::from(
            "discord",
        )))
        .await
        .unwrap();
    let endpoints = residuum_bus::EndpointRegistry::from_entries([residuum_bus::EndpointEntry {
        id: residuum_bus::EndpointId::from("discord"),
        topic: residuum_bus::TopicId::Endpoint(residuum_bus::EndpointName::from("discord")),
        capabilities: residuum_bus::EndpointCapabilities::INTERACTIVE,
        display_name: "Discord".to_string(),
    }]);
    let _source = endpoints.conversations().register(
        "discord",
        Arc::new(residuum::background::testing::BuildsChannel),
    );
    let mut resources = make_sequenced_resources(vec!["unused"]);
    resources.provider = Box::new(residuum::background::testing::SendsToChatProvider {
        calls: std::sync::atomic::AtomicUsize::new(0),
    });
    resources.tools.register(Box::new(
        residuum::tools::send_message::SendMessageTool::new(
            endpoints,
            bus_handle.publisher(),
            true,
        ),
    ));
    let address = SessionAddress::from("external-discord-sends");

    runtime.spawn(
        sample_conversation_request(address.as_ref()),
        Some(resources),
    );

    let posted = next_event("the session's send_message to its chat", &mut chat).await;
    assert_eq!(posted.conversation.as_deref(), Some("chan-1"));
    assert_eq!(posted.content, "Ivan says the build is green");
    next_idle(&mut session_events, &address, "the turn to end").await;
    bus_barrier(&bus_handle).await;
    assert!(
        final_text.drain().is_empty(),
        "the turn's final text stays in the session"
    );
    // The session logs before it goes idle, so its idle line being captured
    // shows the log capture saw this turn; the absence check below means
    // nothing without it.
    assert!(
        !log.matching("session idle, awaiting further input")
            .is_empty(),
        "the session's log lines from this turn are captured"
    );
    assert!(
        !log.events()
            .iter()
            .any(|e| e.text.contains("without a send_message to its chat")),
        "a turn that sent its chat a message is not reported as silent"
    );
    assert!(runtime.registry().stop(&address));
}
