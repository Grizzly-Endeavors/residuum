//! The agent lifecycle tools against a real host. `agent_create` and
//! `agent_delete` act as the calling agent, and the host publishes the toast
//! event and files nothing in the user inbox; creating, deleting and restoring
//! agents shows up in logs and in the messages agents hand each other.
//!
//! covers: residuum-hub, residuum-gateway, residuum-tools
#![expect(
    clippy::tests_outside_test_module,
    reason = "integration tests live in tests/ directory, not inside #[cfg(test)] modules"
)]
#![expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]

use serde_json::json;

use super::hub_host::{Fixture, create_request, drain_events, model_was_told, user_inbox_files};
use residuum::agent::Tool as _;
use residuum::background::team::{A2aVisibility, Actor, AgentState, LifecycleError, TeamDirectory};
use residuum::hub::directory::AgentDirectory;
use residuum::hub::types::{AgentPatch, HubEvent};
use residuum::inference::ToolError;
use residuum::tools::agent_lifecycle::{AgentCreateTool, AgentDeleteTool};
use residuum_core::testing::log::EventLog;
use residuum_core::testing::wait;

fn lifecycle_tools(hub: &Fixture, as_agent: &str) -> (AgentCreateTool, AgentDeleteTool) {
    let access = residuum::tools::LifecycleAccess::new(hub.services.directory.clone(), as_agent);
    (
        AgentCreateTool::new(access.clone(), residuum::agent::HopCounter::new(0)),
        AgentDeleteTool::new(access),
    )
}

fn providers_of(hub: &Fixture, name: &str) -> String {
    std::fs::read_to_string(
        hub.root
            .path()
            .join(name)
            .join("config")
            .join("providers.toml"),
    )
    .unwrap()
}

#[tokio::test]
async fn agent_create_briefs_a_running_teammate_that_inherits_the_creators_settings() {
    let hub = Fixture::new(&["scout"], "").await;
    hub.host.start("scout").await.unwrap();
    hub.host
        .patch(
            "scout",
            AgentPatch {
                a2a_visibility: Some(A2aVisibility::Public),
                ..AgentPatch::default()
            },
        )
        .await
        .unwrap();
    let mut events = hub.host.subscribe();
    let (create, _) = lifecycle_tools(&hub, "scout");

    let result = create
        .execute(json!({ "name": "nova", "description": "keeps the wiki tidy" }))
        .await
        .unwrap();

    assert!(!result.is_error, "{}", result.output);
    assert!(result.output.contains("'nova'"), "{}", result.output);
    assert!(result.output.contains("running"), "{}", result.output);
    assert!(result.output.contains("agent:nova"), "{}", result.output);
    let nova = hub.host.summary("nova").unwrap();
    assert_eq!(nova.state, AgentState::Running);
    assert_eq!(
        nova.a2a_visibility,
        A2aVisibility::Public,
        "the teammate takes the creator's visibility"
    );
    assert_eq!(providers_of(&hub, "nova"), providers_of(&hub, "scout"));
    wait::until("the description to reach the new agent", || async {
        model_was_told(hub.mock("scout"), "keeps the wiki tidy")
            .await
            .then_some(())
    })
    .await;

    assert!(
        drain_events(&mut events).iter().any(|event| matches!(
            event,
            HubEvent::AgentCreated { agent, by: Actor::Agent(creator) }
                if agent.name == "nova" && creator == "scout"
        )),
        "the toast event names the creating agent"
    );
    assert!(
        user_inbox_files(&hub, "scout").is_empty(),
        "creating an agent files nothing in the creator's user inbox"
    );
}

#[tokio::test]
async fn agent_create_explains_bad_and_taken_names() {
    let hub = Fixture::new(&["scout"], "").await;
    hub.host.start("scout").await.unwrap();
    let (create, _) = lifecycle_tools(&hub, "scout");

    let invalid = create
        .execute(json!({ "name": "Not A Name!" }))
        .await
        .unwrap();
    let taken = create.execute(json!({ "name": "scout" })).await.unwrap();
    let missing = create.execute(json!({})).await;

    assert!(invalid.is_error);
    assert!(
        invalid
            .output
            .contains("letters, numbers, spaces, hyphens, and apostrophes"),
        "{}",
        invalid.output
    );
    assert!(taken.is_error);
    assert!(
        taken.output.contains("'scout' already exists"),
        "{}",
        taken.output
    );
    assert!(matches!(missing, Err(ToolError::InvalidArguments(_))));
    assert_eq!(hub.host.list().len(), 1);
}

#[tokio::test]
async fn agent_delete_removes_a_teammate_and_reports_the_checkpoint() {
    let hub = Fixture::new(&["atlas", "scout"], "").await;
    hub.host.start_autostart().await;
    let mut events = hub.host.subscribe();
    let (_, delete) = lifecycle_tools(&hub, "atlas");

    let result = delete.execute(json!({ "name": "scout" })).await.unwrap();

    assert!(!result.is_error, "{}", result.output);
    assert!(
        result.output.contains("Deleted agent 'scout'"),
        "{}",
        result.output
    );
    assert!(
        result.output.contains("checkpointed as"),
        "{}",
        result.output
    );
    assert!(result.output.contains("restore"), "{}", result.output);
    assert!(!hub.root.path().join("scout").exists());
    assert!(matches!(
        hub.host.summary("scout"),
        Err(LifecycleError::NotFound(_))
    ));
    assert!(
        drain_events(&mut events).iter().any(|event| matches!(
            event,
            HubEvent::AgentDeleted { name, by: Actor::Agent(by) }
                if name == "scout" && by == "atlas"
        )),
        "the toast event names the deleting agent"
    );
    assert!(
        user_inbox_files(&hub, "atlas").is_empty(),
        "deleting an agent files nothing in the deleter's user inbox"
    );

    let unknown = delete.execute(json!({ "name": "scout" })).await.unwrap();
    assert!(unknown.is_error);
    assert!(
        unknown.output.contains("no agent named 'scout'"),
        "{}",
        unknown.output
    );
}

#[tokio::test]
async fn an_agent_can_delete_itself_and_the_delete_completes_after_the_call_returns() {
    let hub = Fixture::new(&["atlas", "scout"], "").await;
    hub.host.start_autostart().await;
    let mut events = hub.host.subscribe();
    let (_, delete) = lifecycle_tools(&hub, "scout");

    let result = delete.execute(json!({ "name": "scout" })).await.unwrap();

    assert!(!result.is_error, "{}", result.output);
    assert!(result.output.contains("restore"), "{}", result.output);
    wait::until("the self-delete to finish", || async {
        matches!(hub.host.summary("scout"), Err(LifecycleError::NotFound(_))).then_some(())
    })
    .await;
    assert!(!hub.root.path().join("scout").exists());
    assert_eq!(hub.state_of("atlas"), AgentState::Running);
    assert!(
        drain_events(&mut events).iter().any(|event| matches!(
            event,
            HubEvent::AgentDeleted { name, by: Actor::Agent(by) }
                if name == "scout" && by == "scout"
        )),
        "the user sees the agent delete itself"
    );
}

#[tokio::test]
async fn agent_create_during_shutdown_gets_the_shutdown_refusal() {
    let hub = Fixture::new(&["scout"], "").await;
    hub.host.start("scout").await.unwrap();
    let (create, _) = lifecycle_tools(&hub, "scout");

    hub.host.begin_shutdown();
    let refused = create.execute(json!({ "name": "atlas" })).await.unwrap();

    assert!(refused.is_error);
    assert!(
        refused.output.contains("Residuum is shutting down"),
        "{}",
        refused.output
    );
    assert_eq!(hub.host.list().len(), 1);
    assert!(!hub.root.path().join("atlas").exists());
}

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
    let access = residuum::tools::LifecycleAccess::new(hub.services.directory.clone(), "scout");
    let create = AgentCreateTool::new(access, residuum::agent::HopCounter::new(2));

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
    let access = residuum::tools::LifecycleAccess::new(hub.services.directory.clone(), "scout");
    let create = AgentCreateTool::new(access, residuum::agent::HopCounter::new(10_000));
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
