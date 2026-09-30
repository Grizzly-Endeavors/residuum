//! Restoring deleted agents against a real host: the HTTP round trip, the
//! deleted-agent listing, explicit checkpoints, and the races and reuses of
//! a name.

use super::*;
use crate::checkpoints::{CheckpointContext, CheckpointTrigger, RepoKind};

async fn send_json(
    hub: &Fixture,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let mut request = hub.http.request(method, hub.url(path));
    if let Some(payload) = body {
        request = request.json(&payload);
    }
    let response = request.send().await.unwrap();
    let status = response.status().as_u16();
    let reply = response.json().await.unwrap_or(Value::Null);
    (status, reply)
}

async fn create_over_http(hub: &Fixture, name: &str, description: &str) {
    let (status, body) = send_json(
        hub,
        reqwest::Method::POST,
        "/api/hub/agents",
        Some(json!({ "name": name, "models_from": "scout", "description": description })),
    )
    .await;
    assert_eq!(status, 201, "{body}");
}

async fn delete_over_http(hub: &Fixture, name: &str) -> Value {
    let (status, body) = send_json(
        hub,
        reqwest::Method::DELETE,
        &format!("/api/hub/agents/{name}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    body
}

async fn restore_over_http(hub: &Fixture, body: Value) -> (u16, Value) {
    send_json(
        hub,
        reqwest::Method::POST,
        "/api/hub/agents/restore",
        Some(body),
    )
    .await
}

async fn deleted_names(hub: &Fixture) -> Vec<String> {
    let (status, body) =
        send_json(hub, reqwest::Method::GET, "/api/hub/agents/deleted", None).await;
    assert_eq!(status, 200, "{body}");
    array_at(&body, "agents")
        .iter()
        .map(|agent| str_at(agent, "name").to_string())
        .collect()
}

fn dir_of(hub: &Fixture, name: &str) -> std::path::PathBuf {
    hub.root.path().join(name)
}

/// Record a workspace checkpoint of the agent's directory as it is.
async fn checkpoint(hub: &Fixture, name: &str) -> String {
    let engine = hub
        .host
        .checkpoint_engine_for(name, &dir_of(hub, name))
        .unwrap();
    engine
        .checkpoint_workspace_id_before_action(CheckpointContext::system(
            CheckpointTrigger::PreAction,
            "test checkpoint",
        ))
        .await
        .expect("the workspace checkpoint is recorded")
}

fn restore_request(name: &str, checkpoint_id: Option<&str>) -> RestoreAgentRequest {
    RestoreAgentRequest {
        name: name.to_string(),
        checkpoint_id: checkpoint_id.map(str::to_string),
    }
}

#[tokio::test]
async fn deleting_then_restoring_over_http_round_trips_the_whole_agent() {
    let hub = Fixture::new(&["scout"], "").await;
    hub.host.start("scout").await.unwrap();
    create_over_http(&hub, "nova", "keeps the wiki tidy").await;
    let nova = dir_of(&hub, "nova");
    std::fs::write(nova.join("memory").join("notes.md"), "remember this").unwrap();
    std::fs::write(nova.join("SOUL.md"), "nova's own soul").unwrap();
    let (status, _) = send_json(
        &hub,
        reqwest::Method::PATCH,
        "/api/hub/agents/nova",
        Some(json!({ "a2a_visibility": "public" })),
    )
    .await;
    assert_eq!(status, 200);
    let providers = providers_text(&hub, "nova");
    let config = std::fs::read_to_string(config_path(&hub, "nova")).unwrap();
    let role_page_path = hub.host.team_paths().agent_role_page("nova");
    let role_page = format!(
        "{}\n## Responsibilities\n\nWatches the feeds.\n",
        std::fs::read_to_string(&role_page_path).unwrap()
    );
    std::fs::write(&role_page_path, &role_page).unwrap();
    hub.chat("nova", "hello nova").await;

    let deleted = delete_over_http(&hub, "nova").await;
    let checkpoint_id = str_at(&deleted, "checkpoint_id").to_string();
    assert!(!nova.exists());
    assert!(!role_page_path.exists());

    let (listing_status, listing) =
        send_json(&hub, reqwest::Method::GET, "/api/hub/agents/deleted", None).await;
    assert_eq!(listing_status, 200);
    let agents = array_at(&listing, "agents");
    assert_eq!(agents.len(), 1, "{listing}");
    let first_agent = agents.first().unwrap();
    assert_eq!(str_at(first_agent, "name"), "nova");
    assert_eq!(str_at(first_agent, "checkpoint_id"), checkpoint_id);
    let deleted_at: chrono::DateTime<Utc> = str_at(first_agent, "deleted_at").parse().unwrap();
    assert!(
        (Utc::now() - deleted_at).num_seconds().abs() < 60,
        "{deleted_at}"
    );

    let mut events = hub.host.subscribe();
    let (restore_status, restored) = restore_over_http(&hub, json!({ "name": "nova" })).await;

    assert_eq!(restore_status, 201, "{restored}");
    assert_eq!(str_at(&restored, "name"), "nova");
    assert_eq!(str_at(&restored, "state"), "running");
    assert_eq!(str_at(&restored, "a2a_visibility"), "public");
    assert_eq!(restored.get("autostart"), Some(&json!(true)));
    assert_eq!(str_at(&restored, "role"), "keeps the wiki tidy");
    assert_eq!(
        std::fs::read_to_string(nova.join("memory").join("notes.md")).unwrap(),
        "remember this"
    );
    assert_eq!(
        std::fs::read_to_string(nova.join("SOUL.md")).unwrap(),
        "nova's own soul"
    );
    assert_eq!(providers_text(&hub, "nova"), providers);
    assert_eq!(
        std::fs::read_to_string(config_path(&hub, "nova")).unwrap(),
        config
    );
    assert_eq!(
        std::fs::read_to_string(&role_page_path).unwrap(),
        role_page,
        "the role page comes back as it was"
    );
    let index = std::fs::read_to_string(hub.host.team_paths().wiki_agents_index_md()).unwrap();
    assert!(
        index.contains("[nova](/agents/nova.md) — keeps the wiki tidy"),
        "{index}"
    );
    assert_eq!(hub.chat("nova", "are you back?").await, "scout here");
    let (_, history) = hub.get("/api/agents/nova/chat/history").await;
    assert!(history.contains("hello nova"), "{history}");
    assert!(
        drain_events(&mut events).iter().any(|event| matches!(
            event,
            HubEvent::AgentRestored { agent, by: Actor::User } if agent.name == "nova"
        )),
        "the restore is published with who did it"
    );
    assert!(
        deleted_names(&hub).await.is_empty(),
        "a restored agent is no longer listed"
    );
}

fn providers_text(hub: &Fixture, name: &str) -> String {
    std::fs::read_to_string(providers_path(hub, name)).unwrap()
}

#[tokio::test]
async fn a_restored_agent_starts_only_when_its_settings_say_to() {
    let hub = Fixture::new(&["scout"], "").await;
    create_over_http(&hub, "nova", "keeps the wiki tidy").await;
    hub.host
        .patch(
            "nova",
            AgentPatch {
                autostart: Some(false),
                ..AgentPatch::default()
            },
        )
        .await
        .unwrap();
    delete_over_http(&hub, "nova").await;

    let (status, restored) = restore_over_http(&hub, json!({ "name": "nova" })).await;

    assert_eq!(status, 201, "{restored}");
    assert_eq!(str_at(&restored, "state"), "stopped");
    assert_eq!(restored.get("autostart"), Some(&json!(false)));
}

#[tokio::test]
async fn restoring_from_an_explicit_checkpoint_brings_back_that_state() {
    let hub = Fixture::new(&["scout"], "").await;
    create_over_http(&hub, "nova", "keeps the wiki tidy").await;
    let notes = dir_of(&hub, "nova").join("memory").join("notes.md");
    std::fs::write(&notes, "first version").unwrap();
    let first = checkpoint(&hub, "nova").await;
    std::fs::write(&notes, "second version").unwrap();
    delete_over_http(&hub, "nova").await;

    let (status, body) =
        restore_over_http(&hub, json!({ "name": "nova", "checkpoint_id": first })).await;

    assert_eq!(status, 201, "{body}");
    assert_eq!(std::fs::read_to_string(&notes).unwrap(), "first version");
}

#[tokio::test]
async fn restoring_from_a_checkpoint_the_agent_does_not_have_is_a_bad_request() {
    let hub = Fixture::new(&["scout"], "").await;
    create_over_http(&hub, "nova", "keeps the wiki tidy").await;
    delete_over_http(&hub, "nova").await;

    let (status, body) =
        restore_over_http(&hub, json!({ "name": "nova", "checkpoint_id": "0123abcd" })).await;

    assert_eq!(status, 400, "{body}");
    assert!(str_at(&body, "error").contains("0123abcd"), "{body}");
    assert!(!dir_of(&hub, "nova").exists(), "nothing was written");
    assert_eq!(deleted_names(&hub).await, ["nova"]);
}

#[tokio::test]
async fn restore_refuses_names_that_exist_have_no_history_or_are_invalid() {
    let hub = Fixture::new(&["scout"], "").await;
    create_over_http(&hub, "nova", "keeps the wiki tidy").await;
    delete_over_http(&hub, "nova").await;

    let (taken, taken_body) = restore_over_http(&hub, json!({ "name": "scout" })).await;
    let (unknown, unknown_body) = restore_over_http(&hub, json!({ "name": "ghost" })).await;
    let (invalid, _) = restore_over_http(&hub, json!({ "name": "Not A Name" })).await;
    let (malformed, _) = restore_over_http(&hub, json!({ "nom": "nova" })).await;

    assert_eq!(taken, 409, "{taken_body}");
    assert!(str_at(&taken_body, "error").contains("already exists"));
    assert_eq!(unknown, 404, "{unknown_body}");
    assert!(str_at(&unknown_body, "error").contains("ghost"));
    assert_eq!(invalid, 400);
    assert_eq!(malformed, 400);
    assert_eq!(hub.host.list().len(), 1, "only scout exists");
    assert_eq!(deleted_names(&hub).await, ["nova"]);

    let (first, _) = restore_over_http(&hub, json!({ "name": "nova" })).await;
    let (again, _) = restore_over_http(&hub, json!({ "name": "nova" })).await;
    assert_eq!((first, again), (201, 409), "a second restore is refused");
}

#[tokio::test]
async fn nothing_is_restored_once_the_hub_is_shutting_down() {
    let hub = Fixture::new(&["scout"], "").await;
    create_over_http(&hub, "nova", "keeps the wiki tidy").await;
    delete_over_http(&hub, "nova").await;
    hub.host.begin_shutdown();

    let refused = hub
        .host
        .restore(restore_request("nova", None), Actor::User)
        .await;

    assert!(
        matches!(&refused, Err(LifecycleError::Failed(message)) if message.contains("shutting down")),
        "{refused:?}"
    );
    assert!(!dir_of(&hub, "nova").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restore_racing_a_create_of_the_same_name_leaves_one_consistent_agent() {
    let hub = Fixture::new(&["scout"], "").await;
    create_over_http(&hub, "nova", "the original").await;
    std::fs::write(dir_of(&hub, "nova").join("memory").join("notes.md"), "old").unwrap();
    delete_over_http(&hub, "nova").await;

    let (created, restored) = tokio::join!(
        hub.host
            .create(create_request("nova", Some("the newcomer")), Actor::User),
        hub.host.restore(restore_request("nova", None), Actor::User),
    );

    let outcomes = (created.is_ok(), restored.is_ok());
    assert!(
        outcomes == (true, false) || outcomes == (false, true),
        "exactly one wins: {created:?} / {restored:?}"
    );
    let loser = created.err().or(restored.err()).unwrap();
    assert_eq!(loser, LifecycleError::AlreadyExists("nova".to_string()));
    let names: Vec<String> = hub.host.list().into_iter().map(|a| a.name).collect();
    assert_eq!(names, ["nova", "scout"]);
    let nova = dir_of(&hub, "nova");
    let hub_config = HubConfig::load_at(&hub.services.hub_dir).unwrap();
    assert!(crate::config::Config::load_agent_at(&nova, &hub_config).is_ok());
    assert!(hub.host.team_paths().agent_role_page("nova").is_file());
    assert_eq!(hub.state_of("nova"), AgentState::Running);
    assert!(deleted_names(&hub).await.is_empty());
    assert_eq!(hub.chat("nova", "hello").await, "scout here");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restore_racing_a_delete_of_the_same_name_never_resurrects_a_forgotten_slot() {
    let hub = Fixture::new(&["scout"], "").await;
    create_over_http(&hub, "nova", "keeps the wiki tidy").await;
    delete_over_http(&hub, "nova").await;
    hub.host
        .restore(restore_request("nova", None), Actor::User)
        .await
        .unwrap();

    let (deleted, restored) = tokio::join!(
        hub.host.delete("nova", Actor::User),
        hub.host.restore(restore_request("nova", None), Actor::User),
    );

    // Whichever order they ran in, the hub and the disk agree.
    let listed = hub.host.summary("nova").is_ok();
    assert_eq!(
        listed,
        dir_of(&hub, "nova")
            .join("config")
            .join("config.toml")
            .is_file(),
        "{deleted:?} / {restored:?}"
    );
    assert_eq!(
        deleted_names(&hub).await.contains(&"nova".to_string()),
        !listed
    );
}

#[tokio::test]
async fn the_deleted_list_names_only_agents_without_a_directory_newest_first() {
    let hub = Fixture::new(&["scout"], "").await;
    create_over_http(&hub, "nova", "keeps the wiki tidy").await;
    create_over_http(&hub, "kit", "answers questions").await;
    assert!(
        deleted_names(&hub).await.is_empty(),
        "nothing is deleted yet"
    );

    delete_over_http(&hub, "nova").await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    delete_over_http(&hub, "kit").await;

    assert_eq!(deleted_names(&hub).await, ["kit", "nova"]);
    hub.host
        .restore(restore_request("kit", None), Actor::User)
        .await
        .unwrap();
    assert_eq!(deleted_names(&hub).await, ["nova"]);
    delete_over_http(&hub, "kit").await;
    assert_eq!(
        deleted_names(&hub).await,
        ["kit", "nova"],
        "an agent deleted again is listed again"
    );
}

#[tokio::test]
async fn a_deletion_without_a_record_is_listed_at_its_last_checkpoint() {
    let hub = Fixture::new(&["scout"], "").await;
    create_over_http(&hub, "nova", "").await;
    let deleted = delete_over_http(&hub, "nova").await;
    let checkpoints_dir = hub.host.checkpoints_dir();
    std::fs::remove_file(
        crate::checkpoints::agent_repos_dir(&checkpoints_dir, "nova").join("deleted.json"),
    )
    .unwrap();

    let listed = hub.host.list_deleted().await.unwrap();

    assert_eq!(listed.len(), 1);
    assert_eq!(
        listed.first().unwrap().checkpoint_id,
        str_at(&deleted, "checkpoint_id")
    );
    let (status, restored) = restore_over_http(&hub, json!({ "name": "nova" })).await;
    assert_eq!(status, 201, "{restored}");
    assert!(
        str_at(&restored, "role").contains("Role not described yet"),
        "without the record the role page has the placeholder role: {restored}"
    );
}

#[tokio::test]
async fn recreating_a_deleted_name_keeps_the_old_history_intact_and_restores_the_newest() {
    let hub = Fixture::new(&["scout"], "").await;
    create_over_http(&hub, "nova", "first life").await;
    let first_file = dir_of(&hub, "nova").join("memory").join("first.md");
    std::fs::write(&first_file, "from the first life").unwrap();
    let first_deletion = delete_over_http(&hub, "nova").await;
    let first_checkpoint = str_at(&first_deletion, "checkpoint_id").to_string();

    // A new agent with the same name reuses the name-keyed history.
    create_over_http(&hub, "nova", "second life").await;
    assert!(
        deleted_names(&hub).await.is_empty(),
        "an existing agent is not listed"
    );
    let second_file = dir_of(&hub, "nova").join("memory").join("second.md");
    std::fs::write(&second_file, "from the second life").unwrap();
    let mid_life = checkpoint(&hub, "nova").await;
    let second_deletion = delete_over_http(&hub, "nova").await;
    let second_checkpoint = str_at(&second_deletion, "checkpoint_id").to_string();
    assert_ne!(first_checkpoint, second_checkpoint);

    // Both lives are in one linear history and every checkpoint reads back.
    let engine = hub
        .host
        .checkpoint_engine_for("nova", &dir_of(&hub, "nova"))
        .unwrap();
    let page = engine
        .list_checkpoints(RepoKind::Workspace, None, None, None, Some(100))
        .await
        .unwrap();
    let ids: Vec<&str> = page.items.iter().map(|c| c.id.as_str()).collect();
    for id in [&first_checkpoint, &mid_life, &second_checkpoint] {
        assert!(ids.contains(&id.as_str()), "{id} is in {ids:?}");
        engine
            .show_checkpoint(RepoKind::Workspace, id.clone())
            .await
            .unwrap();
    }
    let listed = hub.host.list_deleted().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed.first().unwrap().checkpoint_id, second_checkpoint);

    // The default restore is the newest life; the first is one id away.
    let (newest_status, _) = restore_over_http(&hub, json!({ "name": "nova" })).await;
    assert_eq!(newest_status, 201);
    assert_eq!(
        std::fs::read_to_string(&second_file).unwrap(),
        "from the second life"
    );
    assert!(!first_file.exists(), "the first life is not mixed in");
    delete_over_http(&hub, "nova").await;
    let (older_status, _) = restore_over_http(
        &hub,
        json!({ "name": "nova", "checkpoint_id": first_checkpoint }),
    )
    .await;
    assert_eq!(older_status, 201);
    assert_eq!(
        std::fs::read_to_string(&first_file).unwrap(),
        "from the first life"
    );
}

#[tokio::test]
async fn an_agent_that_restores_another_gets_an_inbox_item_and_the_event_names_it() {
    let hub = Fixture::new(&["scout"], "").await;
    hub.host.start("scout").await.unwrap();
    create_over_http(&hub, "nova", "keeps the wiki tidy").await;
    delete_over_http(&hub, "nova").await;
    let inbox = WorkspaceLayout::new(hub.root.path().join("scout")).user_inbox_dir();
    let items = || std::fs::read_dir(&inbox).map_or(0, Iterator::count);
    let before = items();
    let mut events = hub.host.subscribe();

    hub.host
        .restore(
            restore_request("nova", None),
            Actor::Agent("scout".to_string()),
        )
        .await
        .unwrap();

    assert_eq!(items(), before + 1);
    assert!(
        drain_events(&mut events).iter().any(|event| matches!(
            event,
            HubEvent::AgentRestored { agent, by: Actor::Agent(actor) }
                if agent.name == "nova" && actor == "scout"
        )),
        "the restore is published with the acting agent"
    );
}
