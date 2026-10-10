//! Lifecycle behaviour around creating, deleting and restoring agents that
//! shows up in logs and in the messages agents hand each other.

use super::*;
use crate::hub::test_support::EventLog;
use crate::tools::Tool as _;
use crate::tools::agent_lifecycle::AgentCreateTool;

#[tokio::test]
async fn a_refused_start_after_creation_or_restore_is_logged_with_its_reason() {
    let hub = Fixture::new(&["scout"], "").await;
    let slot = hub.host.slot("scout").unwrap();
    let log = EventLog::default();
    let _guard = log.capture();

    hub.host.note_start_outcome(
        &slot,
        &Err(LifecycleError::Failed(
            "the hub is shutting down".to_string(),
        )),
    );
    hub.host.note_start_outcome(&slot, &Ok(()));

    let logged = log.matching("agent was not started");
    let [event] = logged.as_slice() else {
        panic!("expected one info line for the refusal only, got {logged:?}");
    };
    assert_eq!(event.level, tracing::Level::INFO);
    assert!(event.text.contains("agent=scout"), "{}", event.text);
    assert!(event.text.contains("shutting down"), "{}", event.text);
    assert!(event.text.contains("state=stopped"), "{}", event.text);
    assert_eq!(
        hub.host.summary("scout").unwrap().state,
        AgentState::Stopped
    );
}

#[tokio::test]
async fn a_user_created_agent_gets_its_role_as_the_owners_own_message() {
    let hub = Fixture::new(&["scout"], "").await;

    hub.host
        .create(
            create_request("nova", Some("keeps the wiki tidy")),
            Actor::User,
        )
        .await
        .unwrap();

    wait::until("the description to reach the new agent", || async {
        model_was_told(hub.mock("scout"), "keeps the wiki tidy")
            .await
            .then_some(())
    })
    .await;
    assert!(
        !model_was_told(hub.mock("scout"), "Message from teammate").await,
        "the owner's message is not framed as a teammate's"
    );
}

#[tokio::test]
async fn an_agent_created_by_an_agent_gets_its_role_as_a_teammate_message() {
    let hub = Fixture::new(&["scout"], "").await;
    hub.host.start("scout").await.unwrap();
    let access = crate::tools::LifecycleAccess::new(hub.services.directory.clone(), "scout");
    let create = AgentCreateTool::new(access, crate::agent::HopCounter::new(2));

    let result = create
        .execute(json!({ "name": "nova", "description": "keeps the wiki tidy" }))
        .await
        .unwrap();

    assert!(!result.is_error, "{}", result.output);
    wait::until(
        "the role to arrive framed as a teammate message",
        || async {
            model_was_told(hub.mock("scout"), "Message from teammate agent:scout")
                .await
                .then_some(())
        },
    )
    .await;
}

#[tokio::test]
async fn a_create_chain_stops_at_the_hop_limit() {
    let hub = Fixture::new(&["scout"], "").await;
    hub.host.start("scout").await.unwrap();
    let access = crate::tools::LifecycleAccess::new(hub.services.directory.clone(), "scout");
    let create = AgentCreateTool::new(access, crate::agent::HopCounter::new(10_000));
    let mut events = hub.host.subscribe();

    let result = create
        .execute(json!({ "name": "nova", "description": "keeps the wiki tidy" }))
        .await
        .unwrap();

    assert!(
        !result.is_error,
        "the agent is still created: {}",
        result.output
    );
    assert!(hub.host.summary("nova").is_ok());
    // The notice is sent only after the delivery has been refused, so once it
    // arrives, nothing is on its way to the creator's model.
    wait::next_matching(
        "the refusal to be shown to the user",
        &mut events,
        |event| {
            matches!(
                event,
                HubEvent::Notice { message, .. }
                    if message.contains("role description couldn't be delivered")
            )
        },
    )
    .await;
    assert!(
        !model_was_told(hub.mock("scout"), "keeps the wiki tidy").await,
        "a message past the hop limit is not delivered"
    );
}
