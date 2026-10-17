//! Write coordination in the `team/` namespace between the hub's file routes
//! and agents' file tools: a web save, delete, restore, undo or workbench
//! delete and an agent's read and write of the same team file see each
//! other's changes, and whoever loses is told who changed it.
//!
//! covers: residuum-hub, residuum-tools, residuum-checkpoints, residuum-workspace

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
use serde_json::{Value, json};
use tower::ServiceExt as _;

use crate::agent::ToolRegistry;
use crate::checkpoints::{CheckpointContext, CheckpointEngine, CheckpointTrigger, RepoKind};
use crate::config::paths::TeamPaths;
use crate::hub::web::workbench::{WorkbenchApiState, workbench_api_router};
use crate::hub::web::{ConfigApiState, WorkspaceScope, agent_repair_api_router};
use crate::inference::ToolResult;
use crate::tools::testing::tools_for;
use crate::tunnel::TunnelStatus;
use crate::workbench::WorkbenchServing;
use crate::workspace::team_files::TeamWriter;
use crate::workspace::testing::TeamHub;
use crate::workspace::version::version_token;

/// The agent `scout`'s repair routes over a hub with a team directory, as the
/// web reaches them, and the checkpoint engine behind them.
struct Fixture {
    hub: TeamHub,
    routes: axum::Router,
    checkpoints: Arc<CheckpointEngine>,
}

fn fixture() -> Fixture {
    let hub = TeamHub::new();
    let agent_dir = hub.agent_dir("scout");
    let hub_dir = hub.dir.path().join("hub");
    std::fs::create_dir_all(&hub_dir).unwrap();
    // A real engine: its team repository watches the hub's team directory, so
    // the checkpoints the routes return can be restored from.
    let checkpoints = Arc::new(
        CheckpointEngine::new(
            "test-agent",
            agent_dir.clone(),
            &TeamPaths::new(hub.team()),
            agent_dir.join("config"),
            hub_dir.clone(),
            &hub_dir.join("checkpoints"),
            None,
        )
        .unwrap()
        .with_team_coordinator(hub.coordinator.clone()),
    );
    let routes = agent_repair_api_router(ConfigApiState {
        team: Some(hub.coordinator.view_for_user(&agent_dir)),
        hub_dir,
        config_dir: agent_dir.join("config"),
        agent_name: "scout".to_string(),
        workspace_dir: agent_dir,
        memory_dir: None,
        reload_tx: None,
        scope: WorkspaceScope::Agent,
        checkpoints: Arc::clone(&checkpoints),
    });
    Fixture {
        hub,
        routes,
        checkpoints,
    }
}

/// `value[key]`, panicking with the missing key named.
fn field<'a>(value: &'a Value, key: &str) -> &'a Value {
    value
        .get(key)
        .unwrap_or_else(|| panic!("missing field {key} in {value}"))
}

async fn body_json(response: Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// `PUT /api/workspace/file` with `content` at `path`, conditional on
/// `if_match` when given.
async fn put(f: &Fixture, path: &str, content: &str, if_match: Option<&str>) -> Response {
    let mut request =
        Request::put("/api/workspace/file").header(header::CONTENT_TYPE, "application/json");
    if let Some(version) = if_match {
        request = request.header(header::IF_MATCH, version);
    }
    let body = json!({ "path": path, "content": content }).to_string();
    f.routes
        .clone()
        .oneshot(request.body(Body::from(body)).unwrap())
        .await
        .unwrap()
}

/// `DELETE /api/workspace/file` of `path`, recursively when `recursive`.
async fn delete_file(f: &Fixture, path: &str, recursive: bool) -> Response {
    f.routes
        .clone()
        .oneshot(
            Request::delete(format!(
                "/api/workspace/file?path={path}&recursive={recursive}"
            ))
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap()
}

/// Delete `path` recursively, which must succeed, and return the response.
async fn delete(f: &Fixture, path: &str) -> Value {
    let response = delete_file(f, path, true).await;
    assert_eq!(response.status(), StatusCode::OK, "action should succeed");
    body_json(response).await
}

fn version_of(path: &std::path::Path) -> String {
    version_token(&std::fs::metadata(path).unwrap())
}

/// The `(id, repo)` of the single checkpoint a response names, checking the
/// response uses the one-checkpoint shape.
fn single_checkpoint(body: &Value) -> (String, String) {
    assert!(
        body.get("checkpoints").is_none(),
        "one repository means the flat shape: {body}"
    );
    (
        field(body, "checkpoint_id").as_str().unwrap().to_string(),
        field(body, "checkpoint_repo").as_str().unwrap().to_string(),
    )
}

/// Restore `path` (relative to the repository's root) from checkpoint `id`.
async fn restore(f: &Fixture, repo: RepoKind, id: &str, path: &str) {
    f.checkpoints
        .restore_path(
            repo,
            id.to_string(),
            path.to_string(),
            CheckpointContext::system(CheckpointTrigger::Restore, "test restore"),
            &TeamWriter::User,
        )
        .await
        .unwrap();
}

fn write_team(f: &Fixture, rel: &str, content: &str) {
    let path = rel
        .split('/')
        .fold(f.hub.team(), |acc, part| acc.join(part));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

async fn tool_call(tools: &ToolRegistry, name: &str, args: Value) -> ToolResult {
    tools.execute(name, args).await.unwrap()
}

#[tokio::test]
async fn an_agents_write_makes_a_stale_web_save_answer_412() {
    let f = fixture();
    std::fs::write(f.hub.team().join("shared.md"), "start").unwrap();
    let path = f.hub.team().join("shared.md");
    let version_seen_by_the_browser = version_of(&path);

    let sam = tools_for(&f.hub, "sam");
    sam.execute("read_file", json!({ "path": "team/shared.md" }))
        .await
        .unwrap();
    let written = sam
        .execute(
            "write_file",
            json!({ "path": "team/shared.md", "content": "sam's version" }),
        )
        .await
        .unwrap();
    assert!(!written.is_error, "{}", written.output);

    let response = put(
        &f,
        "team/shared.md",
        "browser version",
        Some(&version_seen_by_the_browser),
    )
    .await;
    assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "sam's version");
}

#[tokio::test]
async fn a_web_save_makes_a_stale_agent_write_name_the_user() {
    let f = fixture();
    std::fs::write(f.hub.team().join("shared.md"), "start").unwrap();
    let sam = tools_for(&f.hub, "sam");
    sam.execute("read_file", json!({ "path": "team/shared.md" }))
        .await
        .unwrap();

    let response = put(&f, "team/shared.md", "browser version", None).await;
    assert_eq!(response.status(), StatusCode::OK);

    let refused = sam
        .execute(
            "write_file",
            json!({ "path": "team/shared.md", "content": "sam's version" }),
        )
        .await
        .unwrap();
    assert!(refused.is_error);
    assert!(refused.output.contains("the user"), "{}", refused.output);
}

#[tokio::test]
async fn a_web_delete_is_reported_to_an_agent_that_read_the_file() {
    let f = fixture();
    std::fs::write(f.hub.team().join("shared.md"), "start").unwrap();
    let sam = tools_for(&f.hub, "sam");
    sam.execute("read_file", json!({ "path": "team/shared.md" }))
        .await
        .unwrap();

    let deleted = delete_file(&f, "team/shared.md", false).await;
    assert_eq!(deleted.status(), StatusCode::OK);

    let refused = sam
        .execute(
            "write_file",
            json!({ "path": "team/shared.md", "content": "sam's version" }),
        )
        .await
        .unwrap();
    assert!(refused.is_error);
    assert!(refused.output.contains("the user"), "{}", refused.output);
}

#[tokio::test]
async fn a_web_save_and_an_agent_write_racing_leave_one_winner() {
    let f = fixture();
    let path = f.hub.team().join("shared.md");
    std::fs::write(&path, "start").unwrap();
    let version = version_of(&path);
    let sam = tools_for(&f.hub, "sam");
    sam.execute("read_file", json!({ "path": "team/shared.md" }))
        .await
        .unwrap();

    let (web, tool) = tokio::join!(
        put(&f, "team/shared.md", "browser version", Some(&version)),
        sam.execute(
            "write_file",
            json!({ "path": "team/shared.md", "content": "sam's version" }),
        ),
    );
    let tool = tool.unwrap();

    let web_won = web.status() == StatusCode::OK;
    assert_ne!(
        web_won,
        !tool.is_error,
        "exactly one writer succeeds: web {}, tool error {}",
        web.status(),
        tool.is_error
    );
    if web_won {
        assert!(tool.output.contains("the user"), "{}", tool.output);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "browser version");
    } else {
        assert_eq!(web.status(), StatusCode::PRECONDITION_FAILED);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "sam's version");
    }
}

#[tokio::test]
async fn a_restore_into_team_is_recorded_as_the_user_and_conflicts_a_stale_reader() {
    let f = fixture();
    write_team(&f, "wiki/a.md", "shared page");
    let file = f.hub.team().join("wiki").join("a.md");
    let body = delete(&f, "team/wiki/a.md").await;
    let (id, _) = single_checkpoint(&body);

    // Sam sees the file gone, then the user restores it.
    let sam = tools_for(&f.hub, "sam");
    let missing = tool_call(&sam, "read_file", json!({ "path": "team/wiki/a.md" })).await;
    assert!(missing.is_error);
    restore(&f, RepoKind::Team, &id, "wiki/a.md").await;
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "shared page");

    let refused = tool_call(
        &sam,
        "write_file",
        json!({ "path": "team/wiki/a.md", "content": "sam" }),
    )
    .await;
    assert!(refused.is_error, "{}", refused.output);
    assert!(refused.output.contains("the user"), "{}", refused.output);
}

#[tokio::test]
async fn an_undo_in_team_locks_and_names_the_agent_that_ran_it() {
    let f = fixture();
    write_team(&f, "wiki/a.md", "one");
    let ctx = |summary: &str| CheckpointContext::system(CheckpointTrigger::PreAction, summary);
    f.checkpoints
        .checkpoint_team_before_action(ctx("first"))
        .await;
    write_team(&f, "wiki/a.md", "two");
    let second = f
        .checkpoints
        .checkpoint_team_id_before_action(ctx("second"))
        .await
        .unwrap();

    let sam = tools_for(&f.hub, "sam");
    assert!(
        !tool_call(&sam, "read_file", json!({ "path": "team/wiki/a.md" }))
            .await
            .is_error
    );
    f.checkpoints
        .undo_checkpoint(
            RepoKind::Team,
            second,
            CheckpointContext::system(CheckpointTrigger::Undo, "undo"),
            &TeamWriter::Agent("robin".to_string()),
        )
        .await
        .unwrap();
    let file = f.hub.team().join("wiki").join("a.md");
    assert_eq!(std::fs::read_to_string(file).unwrap(), "one");

    let refused = tool_call(
        &sam,
        "write_file",
        json!({ "path": "team/wiki/a.md", "content": "sam" }),
    )
    .await;
    assert!(refused.is_error, "{}", refused.output);
    assert!(refused.output.contains("robin"), "{}", refused.output);
}

#[tokio::test]
async fn a_workbench_delete_takes_the_path_locks_and_records_the_user() {
    let hub = TeamHub::new();
    let workbench = hub.team().join("workbench");
    std::fs::create_dir_all(&workbench).unwrap();
    std::fs::write(workbench.join("chart.html"), "<title>Chart</title>").unwrap();
    std::fs::write(workbench.join("chart.state.json"), "{}").unwrap();
    let sam = tools_for(&hub, "sam");
    let read = sam
        .execute(
            "read_file",
            serde_json::json!({ "path": "team/workbench/chart.html" }),
        )
        .await
        .unwrap();
    assert!(!read.is_error, "{}", read.output);

    let (_tx, rx) = tokio::sync::watch::channel(TunnelStatus::Disconnected);
    let router = workbench_api_router(WorkbenchApiState {
        dir: workbench.clone(),
        serving: WorkbenchServing::Running { port: 7702 },
        tunnel_status_rx: rx,
        checkpoints: crate::checkpoints::testing::test_engine(),
        team: Some(hub.coordinator.view_for_user(hub.agent_dir("scout"))),
    });

    let held = hub
        .coordinator
        .lock(&workbench.join("chart.state.json"))
        .await;
    let deleting = router.oneshot(
        Request::delete("/api/team/workbench/artifacts/chart")
            .body(Body::empty())
            .unwrap(),
    );
    tokio::pin!(deleting);
    let data_file = workbench.join("chart.state.json");
    tokio::select! {
        _ = &mut deleting => panic!("the delete must wait for the data file's lock"),
        () = residuum_core::testing::wait::until_true("the delete to queue for the data file's lock", || {
            hub.coordinator.lock_contenders(&data_file) == 2
        }) => {}
    }
    assert!(workbench.join("chart.html").exists());

    drop(held);
    let response = deleting.await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!workbench.join("chart.html").exists());

    let refused = sam
        .execute(
            "write_file",
            serde_json::json!({ "path": "team/workbench/chart.html", "content": "again" }),
        )
        .await
        .unwrap();
    assert!(refused.is_error, "{}", refused.output);
    assert!(refused.output.contains("the user"), "{}", refused.output);
}
