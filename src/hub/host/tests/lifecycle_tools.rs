//! `agent_create` and `agent_delete` against a real host: the tools act as
//! the calling agent, and the host publishes the toast event and files
//! nothing in the user inbox.

use super::*;
use crate::tools::Tool as _;
use crate::tools::ToolError;
use crate::tools::agent_lifecycle::{AgentCreateTool, AgentDeleteTool};

fn lifecycle_tools(hub: &Fixture, as_agent: &str) -> (AgentCreateTool, AgentDeleteTool) {
    let access = crate::tools::LifecycleAccess::new(hub.services.directory.clone(), as_agent);
    (
        AgentCreateTool::new(access.clone(), crate::agent::HopCounter::new(0)),
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
    eventually("the description to reach the new agent", || async {
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
        .execute(json!({ "name": "Not A Name" }))
        .await
        .unwrap();
    let taken = create.execute(json!({ "name": "scout" })).await.unwrap();
    let missing = create.execute(json!({})).await;

    assert!(invalid.is_error);
    assert!(
        invalid
            .output
            .contains("lowercase letters, digits, and hyphens"),
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
    eventually("the self-delete to finish", || async {
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
