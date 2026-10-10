//! The `team/` namespace in the workspace file API, and write coordination
//! between the API and agents' file tools.

use std::path::PathBuf;

use axum::body::Bytes;
use axum::extract::{Query, RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Json, Response};
use serde_json::{Value, json};

use super::ConfigApiState;
use super::workspace::{
    DeleteFileQuery, FileQuery, FilesQuery, MkdirRequest, MoveRequest, WriteFileRequest,
    api_workspace_delete, api_workspace_file_read, api_workspace_file_write, api_workspace_files,
    api_workspace_mkdir, api_workspace_move, api_workspace_raw_read, api_workspace_raw_write,
};
use super::workspace_bulk::{api_workspace_read, api_workspace_tree};
use crate::tools::team_namespace_tests::Hub;
use crate::workspace::version::version_token;

struct Fixture {
    hub: Hub,
    agent_dir: PathBuf,
    state: ConfigApiState,
}

fn fixture() -> Fixture {
    let hub = Hub::new();
    let agent_dir = hub.agent_dir("scout");
    let hub_dir = hub.dir.path().join("hub");
    std::fs::create_dir_all(&hub_dir).unwrap();
    // A real engine: its team repository watches the hub's team directory, so
    // the checkpoints the handlers return can be restored from.
    let checkpoints = std::sync::Arc::new(
        crate::checkpoints::CheckpointEngine::new(
            "test-agent",
            agent_dir.clone(),
            &crate::config::paths::TeamPaths::new(hub.team()),
            agent_dir.join("config"),
            hub_dir.clone(),
            &hub_dir.join("checkpoints"),
            None,
        )
        .unwrap()
        .with_team_coordinator(hub.coordinator.clone()),
    );
    let state = ConfigApiState {
        team: Some(hub.coordinator.view_for_user(&agent_dir)),
        hub_dir,
        config_dir: agent_dir.join("config"),
        agent_name: "scout".to_string(),
        workspace_dir: agent_dir.clone(),
        memory_dir: None,
        reload_tx: None,
        scope: crate::gateway::web::WorkspaceScope::Agent,
        checkpoints,
    };
    Fixture {
        hub,
        agent_dir,
        state,
    }
}

async fn body_bytes(response: Response) -> Vec<u8> {
    axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec()
}

/// `value[key]`, panicking with the missing key named.
fn field<'a>(value: &'a Value, key: &str) -> &'a Value {
    value
        .get(key)
        .unwrap_or_else(|| panic!("missing field {key} in {value}"))
}

/// `value[index]`, panicking with the missing index named.
fn item(value: &Value, index: usize) -> &Value {
    value
        .get(index)
        .unwrap_or_else(|| panic!("missing item {index} in {value}"))
}

/// The `path` of every entry in a serialized tree response.
fn entry_paths(tree: &Value) -> Vec<String> {
    field(tree, "entries")
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| field(entry, "path").as_str().unwrap().to_string())
        .collect()
}

async fn list(state: &ConfigApiState, path: Option<&str>) -> Vec<String> {
    let Json(entries) = api_workspace_files(
        Query(FilesQuery {
            path: path.map(str::to_string),
        }),
        State(state.clone()),
    )
    .await
    .unwrap();
    entries
        .into_iter()
        .map(|e| format!("{}:{}", e.entry_type, e.name))
        .collect()
}

async fn put(
    state: &ConfigApiState,
    path: &str,
    content: &str,
    if_match: Option<&str>,
) -> Response {
    let mut headers = HeaderMap::new();
    if let Some(version) = if_match {
        headers.insert(header::IF_MATCH, HeaderValue::from_str(version).unwrap());
    }
    api_workspace_file_write(
        State(state.clone()),
        headers,
        Json(WriteFileRequest {
            path: path.to_string(),
            content: content.to_string(),
        }),
    )
    .await
    .unwrap_or_else(IntoResponse::into_response)
}

fn version_of(path: &std::path::Path) -> String {
    version_token(&std::fs::metadata(path).unwrap())
}

#[tokio::test]
async fn the_root_listing_shows_a_team_folder() {
    let f = fixture();
    std::fs::write(f.agent_dir.join("SOUL.md"), "soul").unwrap();
    std::fs::create_dir_all(f.hub.team().join("wiki")).unwrap();
    std::fs::write(f.hub.team().join("AGENTS.md"), "rules").unwrap();

    let root = list(&f.state, None).await;
    assert!(root.contains(&"directory:team".to_string()), "{root:?}");
    assert!(root.contains(&"file:SOUL.md".to_string()), "{root:?}");

    let team = list(&f.state, Some("team")).await;
    assert!(team.contains(&"directory:wiki".to_string()), "{team:?}");
    assert!(team.contains(&"file:AGENTS.md".to_string()), "{team:?}");
    assert!(
        !team.contains(&"file:SOUL.md".to_string()),
        "the agent's files are not in the team folder: {team:?}"
    );
}

#[tokio::test]
async fn blocked_team_paths_stay_hidden() {
    let f = fixture();
    let team = f.hub.team();
    std::fs::create_dir_all(team.join(".index")).unwrap();
    std::fs::write(team.join("vectors.db"), "db").unwrap();
    std::fs::write(team.join("vectors.db-wal"), "wal").unwrap();
    std::fs::write(team.join(".a.md.0badf00d.residuum-tmp"), "tmp").unwrap();
    std::fs::write(team.join("visible.md"), "ok").unwrap();

    assert_eq!(list(&f.state, Some("team")).await, vec!["file:visible.md"]);

    let err = api_workspace_file_read(
        Query(FileQuery {
            path: "team/vectors.db".to_string(),
        }),
        State(f.state.clone()),
    )
    .await
    .unwrap_err();
    assert_eq!(err.0, StatusCode::FORBIDDEN);
    let response = put(&f.state, "team/.index/x", "no", None).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn team_files_read_as_text_and_raw() {
    let f = fixture();
    std::fs::create_dir_all(f.hub.team().join("wiki")).unwrap();
    std::fs::write(f.hub.team().join("wiki").join("a.md"), "shared page").unwrap();

    let text = api_workspace_file_read(
        Query(FileQuery {
            path: "team/wiki/a.md".to_string(),
        }),
        State(f.state.clone()),
    )
    .await
    .unwrap();
    assert!(text.headers().contains_key(header::ETAG));
    assert_eq!(body_bytes(text).await, b"shared page");

    let raw = api_workspace_raw_read(
        Query(FileQuery {
            path: "team/wiki/a.md".to_string(),
        }),
        State(f.state.clone()),
    )
    .await
    .unwrap();
    assert_eq!(body_bytes(raw).await, b"shared page");

    let missing = api_workspace_file_read(
        Query(FileQuery {
            path: "team/wiki/nope.md".to_string(),
        }),
        State(f.state.clone()),
    )
    .await
    .unwrap_err();
    assert_eq!(missing.0, StatusCode::NOT_FOUND);
    assert!(
        missing.1.contains("team/wiki/nope.md"),
        "errors name the path as the client sent it: {}",
        missing.1
    );
}

#[tokio::test]
async fn team_paths_cannot_escape_the_team_folder() {
    let f = fixture();
    std::fs::write(f.agent_dir.join("SOUL.md"), "soul").unwrap();

    let err = api_workspace_file_read(
        Query(FileQuery {
            path: "team/../scout/SOUL.md".to_string(),
        }),
        State(f.state.clone()),
    )
    .await
    .unwrap_err();
    assert_eq!(err.0, StatusCode::FORBIDDEN);
    let response = put(&f.state, "team/../escaped.md", "no", None).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn writes_to_team_paths_land_in_the_team_folder() {
    let f = fixture();

    let response = put(&f.state, "team/wiki/new.md", "hello", None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        std::fs::read_to_string(f.hub.team().join("wiki").join("new.md")).unwrap(),
        "hello"
    );
    assert!(
        !f.agent_dir.join("team").exists(),
        "the agent folder must not gain a team entry"
    );

    let private_response = put(&f.state, "notes.md", "private", None).await;
    assert_eq!(private_response.status(), StatusCode::OK);
    assert!(f.agent_dir.join("notes.md").exists());
}

#[tokio::test]
async fn the_team_folder_itself_cannot_be_replaced_moved_or_deleted() {
    let f = fixture();
    std::fs::write(f.agent_dir.join("a.md"), "a").unwrap();

    let response = put(&f.state, "team", "no", None).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let onto_team = api_workspace_move(
        State(f.state.clone()),
        HeaderMap::new(),
        Json(MoveRequest {
            from: "a.md".to_string(),
            to: "team".to_string(),
            overwrite: true,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(onto_team.0, StatusCode::BAD_REQUEST);

    let away_from_team = api_workspace_move(
        State(f.state.clone()),
        HeaderMap::new(),
        Json(MoveRequest {
            from: "team".to_string(),
            to: "elsewhere".to_string(),
            overwrite: false,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(away_from_team.0, StatusCode::BAD_REQUEST);

    let delete_team = api_workspace_delete(
        Query(DeleteFileQuery {
            path: "team".to_string(),
            recursive: true,
        }),
        State(f.state.clone()),
        HeaderMap::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(delete_team.0, StatusCode::BAD_REQUEST);
    assert!(f.hub.team().is_dir());
}

#[tokio::test]
async fn mkdir_move_and_delete_work_inside_and_across_the_namespace() {
    let f = fixture();
    let team = f.hub.team();
    std::fs::write(f.agent_dir.join("draft.md"), "draft").unwrap();

    let created = api_workspace_mkdir(
        State(f.state.clone()),
        Json(MkdirRequest {
            path: "team/wiki/people".to_string(),
        }),
    )
    .await
    .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    assert!(team.join("wiki").join("people").is_dir());

    // Agent folder into the team folder.
    let moved = api_workspace_move(
        State(f.state.clone()),
        HeaderMap::new(),
        Json(MoveRequest {
            from: "draft.md".to_string(),
            to: "team/wiki/people/draft.md".to_string(),
            overwrite: false,
        }),
    )
    .await
    .unwrap();
    assert_eq!(moved.status(), StatusCode::OK);
    assert!(team.join("wiki").join("people").join("draft.md").exists());
    assert!(!f.agent_dir.join("draft.md").exists());

    // Within the team folder, then back out.
    let renamed = api_workspace_move(
        State(f.state.clone()),
        HeaderMap::new(),
        Json(MoveRequest {
            from: "team/wiki/people/draft.md".to_string(),
            to: "team/wiki/final.md".to_string(),
            overwrite: false,
        }),
    )
    .await
    .unwrap();
    assert_eq!(renamed.status(), StatusCode::OK);
    let out = api_workspace_move(
        State(f.state.clone()),
        HeaderMap::new(),
        Json(MoveRequest {
            from: "team/wiki/final.md".to_string(),
            to: "final.md".to_string(),
            overwrite: false,
        }),
    )
    .await
    .unwrap();
    assert_eq!(out.status(), StatusCode::OK);
    assert!(f.agent_dir.join("final.md").exists());

    let deleted = api_workspace_delete(
        Query(DeleteFileQuery {
            path: "team/wiki".to_string(),
            recursive: true,
        }),
        State(f.state.clone()),
        HeaderMap::new(),
    )
    .await
    .unwrap();
    assert_eq!(deleted.status(), StatusCode::OK);
    assert!(!team.join("wiki").exists());
}

#[tokio::test]
async fn the_tree_includes_the_team_folder() {
    let f = fixture();
    std::fs::write(f.agent_dir.join("SOUL.md"), "soul").unwrap();
    std::fs::create_dir_all(f.hub.team().join("wiki")).unwrap();
    std::fs::write(f.hub.team().join("wiki").join("a.md"), "page").unwrap();
    std::fs::write(f.hub.team().join("vectors.db"), "db").unwrap();

    let Json(tree) = api_workspace_tree(State(f.state.clone()), RawQuery(None))
        .await
        .unwrap();
    let paths = entry_paths(&serde_json::to_value(&tree).unwrap());
    for expected in ["SOUL.md", "team", "team/wiki", "team/wiki/a.md"] {
        assert!(paths.iter().any(|p| p == expected), "{expected}: {paths:?}");
    }
    assert!(
        !paths.iter().any(|p| p == "team/vectors.db"),
        "the team database stays hidden: {paths:?}"
    );

    let Json(sub) = api_workspace_tree(
        State(f.state.clone()),
        RawQuery(Some("path=team%2Fwiki&content=true".to_string())),
    )
    .await
    .unwrap();
    let sub = serde_json::to_value(&sub).unwrap();
    assert_eq!(field(&sub, "path"), "team/wiki");
    let first = item(field(&sub, "entries"), 0);
    assert_eq!(field(first, "path"), "team/wiki/a.md");
    assert_eq!(field(first, "content"), "page");

    let Json(shallow) = api_workspace_tree(
        State(f.state.clone()),
        RawQuery(Some("depth=1".to_string())),
    )
    .await
    .unwrap();
    let shallow_paths = entry_paths(&serde_json::to_value(&shallow).unwrap());
    assert!(shallow_paths.iter().any(|p| p == "team"));
    assert!(
        !shallow_paths.iter().any(|p| p == "team/wiki"),
        "{shallow_paths:?}"
    );
}

#[tokio::test]
async fn batch_read_serves_team_paths() {
    let f = fixture();
    std::fs::write(f.agent_dir.join("SOUL.md"), "soul").unwrap();
    std::fs::write(f.hub.team().join("AGENTS.md"), "rules").unwrap();
    std::fs::write(f.hub.team().join("vectors.db"), "db").unwrap();

    let request = serde_json::from_value(json!({
        "paths": ["SOUL.md", "team/AGENTS.md", "team/vectors.db", "team/nope.md"]
    }))
    .unwrap();
    let Json(response) = api_workspace_read(State(f.state.clone()), Json(request))
        .await
        .unwrap();
    let response = serde_json::to_value(&response).unwrap();
    let files = field(&response, "files");
    assert_eq!(field(item(files, 0), "content"), "soul");
    assert_eq!(field(item(files, 1), "path"), "team/AGENTS.md");
    assert_eq!(field(item(files, 1), "content"), "rules");
    assert_eq!(field(item(files, 2), "error"), "blocked");
    assert_eq!(field(item(files, 3), "error"), "not_found");
}

#[tokio::test]
async fn an_agents_write_makes_a_stale_web_save_answer_412() {
    let f = fixture();
    std::fs::write(f.hub.team().join("shared.md"), "start").unwrap();
    let path = f.hub.team().join("shared.md");
    let version_seen_by_the_browser = version_of(&path);

    let sam = f.hub.tools_for("sam");
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
        &f.state,
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
    let sam = f.hub.tools_for("sam");
    sam.execute("read_file", json!({ "path": "team/shared.md" }))
        .await
        .unwrap();

    let response = put(&f.state, "team/shared.md", "browser version", None).await;
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
    let sam = f.hub.tools_for("sam");
    sam.execute("read_file", json!({ "path": "team/shared.md" }))
        .await
        .unwrap();

    let deleted = api_workspace_delete(
        Query(DeleteFileQuery {
            path: "team/shared.md".to_string(),
            recursive: false,
        }),
        State(f.state.clone()),
        HeaderMap::new(),
    )
    .await
    .unwrap();
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
    let sam = f.hub.tools_for("sam");
    sam.execute("read_file", json!({ "path": "team/shared.md" }))
        .await
        .unwrap();

    let (web, tool) = tokio::join!(
        put(
            &f.state,
            "team/shared.md",
            "browser version",
            Some(&version)
        ),
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
async fn private_files_are_written_without_coordination() {
    let f = fixture();
    let path = f.agent_dir.join("notes.md");
    std::fs::write(&path, "start").unwrap();

    let unconditional = put(&f.state, "notes.md", "one", None).await;
    assert_eq!(unconditional.status(), StatusCode::OK);
    let conditional = put(&f.state, "notes.md", "two", Some(&version_of(&path))).await;
    assert_eq!(conditional.status(), StatusCode::OK);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "two");
}

#[tokio::test]
async fn saving_the_team_identity_files_signals_a_workspace_reload() {
    let mut f = fixture();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    f.state.reload_tx = Some(tx);

    let identity = put(&f.state, "team/USER.md", "the user", None).await;
    assert_eq!(identity.status(), StatusCode::OK);
    assert_eq!(
        rx.recv().await,
        Some(crate::gateway::types::ReloadSignal::Workspace)
    );

    let other = put(&f.state, "team/wiki/other.md", "not identity", None).await;
    assert_eq!(other.status(), StatusCode::OK);
    assert!(rx.try_recv().is_err(), "other team files signal nothing");
}

// ── Undo: checkpoints for destructive actions on team paths ─────────

use crate::checkpoints::{CheckpointContext, CheckpointTrigger, RepoKind};

async fn json_of(response: Response) -> Value {
    assert_eq!(response.status(), StatusCode::OK, "action should succeed");
    serde_json::from_slice(&body_bytes(response).await).unwrap()
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
    f.state
        .checkpoints
        .restore_path(
            repo,
            id.to_string(),
            path.to_string(),
            CheckpointContext::system(CheckpointTrigger::Restore, "test restore"),
            &crate::workspace::team_files::TeamWriter::User,
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

async fn delete(f: &Fixture, path: &str) -> Value {
    let response = api_workspace_delete(
        Query(DeleteFileQuery {
            path: path.to_string(),
            recursive: true,
        }),
        State(f.state.clone()),
        HeaderMap::new(),
    )
    .await
    .unwrap();
    json_of(response).await
}

async fn move_to(f: &Fixture, from: &str, to: &str, overwrite: bool) -> Value {
    let response = api_workspace_move(
        State(f.state.clone()),
        HeaderMap::new(),
        Json(MoveRequest {
            from: from.to_string(),
            to: to.to_string(),
            overwrite,
        }),
    )
    .await
    .unwrap();
    json_of(response).await
}

#[tokio::test]
async fn deleting_a_team_file_returns_a_team_checkpoint_that_restores_it() {
    let f = fixture();
    write_team(&f, "wiki/a.md", "shared page");
    let file = f.hub.team().join("wiki").join("a.md");

    let body = delete(&f, "team/wiki/a.md").await;
    let (id, repo) = single_checkpoint(&body);
    assert_eq!(repo, "team");
    assert!(!file.exists());

    restore(&f, RepoKind::Team, &id, "wiki/a.md").await;
    assert_eq!(std::fs::read_to_string(file).unwrap(), "shared page");
}

#[tokio::test]
async fn deleting_a_team_directory_returns_a_team_checkpoint_that_restores_it() {
    let f = fixture();
    write_team(&f, "wiki/people/bear.md", "bear");
    write_team(&f, "wiki/people/sam.md", "sam");

    let body = delete(&f, "team/wiki/people").await;
    let (id, repo) = single_checkpoint(&body);
    assert_eq!(repo, "team");

    restore(&f, RepoKind::Team, &id, "wiki/people").await;
    let people = f.hub.team().join("wiki").join("people");
    assert_eq!(
        std::fs::read_to_string(people.join("bear.md")).unwrap(),
        "bear"
    );
    assert_eq!(
        std::fs::read_to_string(people.join("sam.md")).unwrap(),
        "sam"
    );
}

#[tokio::test]
async fn overwriting_a_team_file_with_a_raw_write_returns_a_restorable_team_checkpoint() {
    let f = fixture();
    write_team(&f, "USER.md", "original");
    let file = f.hub.team().join("USER.md");

    let response = api_workspace_raw_write(
        Query(FileQuery {
            path: "team/USER.md".to_string(),
        }),
        State(f.state.clone()),
        HeaderMap::new(),
        Bytes::from_static(b"replacement"),
    )
    .await
    .unwrap();
    let body = json_of(response).await;
    let (id, repo) = single_checkpoint(&body);
    assert_eq!(repo, "team");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "replacement");

    restore(&f, RepoKind::Team, &id, "USER.md").await;
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "original");
}

#[tokio::test]
async fn moving_a_team_file_over_another_returns_a_team_checkpoint_holding_both() {
    let f = fixture();
    write_team(&f, "wiki/old.md", "old");
    write_team(&f, "wiki/new.md", "replaced");
    let old = f.hub.team().join("wiki").join("old.md");
    let new = f.hub.team().join("wiki").join("new.md");

    let body = move_to(&f, "team/wiki/old.md", "team/wiki/new.md", true).await;
    let (id, repo) = single_checkpoint(&body);
    assert_eq!(repo, "team");
    assert!(!old.exists());

    restore(&f, RepoKind::Team, &id, "wiki/old.md").await;
    restore(&f, RepoKind::Team, &id, "wiki/new.md").await;
    assert_eq!(std::fs::read_to_string(old).unwrap(), "old");
    assert_eq!(std::fs::read_to_string(new).unwrap(), "replaced");
}

#[tokio::test]
async fn a_move_between_the_agent_and_team_directories_checkpoints_both_repos() {
    let f = fixture();
    std::fs::write(f.agent_dir.join("draft.md"), "draft").unwrap();
    write_team(&f, "wiki/page.md", "taken over");
    let page = f.hub.team().join("wiki").join("page.md");

    let body = move_to(&f, "draft.md", "team/wiki/page.md", true).await;
    assert!(
        field(&body, "checkpoint_id").is_null(),
        "two checkpoints are listed, not flattened: {body}"
    );
    let listed = field(&body, "checkpoints").as_array().unwrap();
    assert_eq!(listed.len(), 2);
    let id_for = |repo: &str| -> String {
        let entry = listed
            .iter()
            .find(|c| field(c, "checkpoint_repo").as_str() == Some(repo))
            .unwrap_or_else(|| panic!("no {repo} checkpoint in {body}"));
        field(entry, "checkpoint_id").as_str().unwrap().to_string()
    };

    restore(&f, RepoKind::Workspace, &id_for("workspace"), "draft.md").await;
    restore(&f, RepoKind::Team, &id_for("team"), "wiki/page.md").await;
    assert_eq!(
        std::fs::read_to_string(f.agent_dir.join("draft.md")).unwrap(),
        "draft"
    );
    assert_eq!(std::fs::read_to_string(page).unwrap(), "taken over");
}

#[tokio::test]
async fn a_move_from_team_to_the_agent_directory_checkpoints_both_repos() {
    let f = fixture();
    write_team(&f, "wiki/page.md", "shared");
    std::fs::write(f.agent_dir.join("mine.md"), "mine").unwrap();

    let body = move_to(&f, "team/wiki/page.md", "mine.md", true).await;
    let listed = field(&body, "checkpoints").as_array().unwrap();
    let repos: Vec<&str> = listed
        .iter()
        .map(|c| field(c, "checkpoint_repo").as_str().unwrap())
        .collect();
    assert_eq!(repos, ["workspace", "team"]);
}

#[tokio::test]
async fn agent_only_actions_keep_the_workspace_checkpoint_shape() {
    let f = fixture();
    std::fs::write(f.agent_dir.join("gone.md"), "bye").unwrap();
    std::fs::write(f.agent_dir.join("a.md"), "a").unwrap();

    let deleted = delete(&f, "gone.md").await;
    let (id, repo) = single_checkpoint(&deleted);
    assert_eq!(repo, "workspace");
    restore(&f, RepoKind::Workspace, &id, "gone.md").await;
    assert_eq!(
        std::fs::read_to_string(f.agent_dir.join("gone.md")).unwrap(),
        "bye"
    );

    let moved = move_to(&f, "a.md", "b.md", false).await;
    let (_, moved_repo) = single_checkpoint(&moved);
    assert_eq!(moved_repo, "workspace");

    let text_write = json_of(put(&f.state, "c.md", "text", None).await).await;
    assert!(
        text_write.get("checkpoint_id").is_none(),
        "a text write takes no checkpoint: {text_write}"
    );
}

// ── Restore and undo go through the team write coordinator ──────────

async fn tool_call(
    tools: &crate::tools::ToolRegistry,
    name: &str,
    args: Value,
) -> crate::tools::ToolResult {
    tools.execute(name, args).await.unwrap()
}

#[tokio::test]
async fn a_restore_into_team_is_recorded_as_the_user_and_conflicts_a_stale_reader() {
    let f = fixture();
    write_team(&f, "wiki/a.md", "shared page");
    let file = f.hub.team().join("wiki").join("a.md");
    let body = delete(&f, "team/wiki/a.md").await;
    let (id, _) = single_checkpoint(&body);

    // Sam sees the file gone, then the user restores it.
    let sam = f.hub.tools_for("sam");
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
async fn a_restore_waits_for_a_write_holding_the_path_lock() {
    let f = fixture();
    write_team(&f, "wiki/a.md", "shared page");
    let file = f.hub.team().join("wiki").join("a.md");
    let body = delete(&f, "team/wiki/a.md").await;
    let (id, _) = single_checkpoint(&body);

    let held = f.hub.coordinator.lock(&file).await;
    let restoring = restore(&f, RepoKind::Team, &id, "wiki/a.md");
    tokio::pin!(restoring);
    tokio::select! {
        () = &mut restoring => panic!("the restore must wait for the lock holder"),
        () = crate::testing::wait::until_true("the restore to queue for the lock", || {
            f.hub.coordinator.lock_contenders(&file) == 2
        }) => {}
    }
    assert!(!file.exists(), "nothing is restored while the lock is held");

    drop(held);
    restoring.await;
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "shared page");
}

#[tokio::test]
async fn an_undo_in_team_locks_and_names_the_agent_that_ran_it() {
    let f = fixture();
    write_team(&f, "wiki/a.md", "one");
    let ctx = |summary: &str| CheckpointContext::system(CheckpointTrigger::PreAction, summary);
    f.state
        .checkpoints
        .checkpoint_team_before_action(ctx("first"))
        .await;
    write_team(&f, "wiki/a.md", "two");
    let second = f
        .state
        .checkpoints
        .checkpoint_team_id_before_action(ctx("second"))
        .await
        .unwrap();

    let sam = f.hub.tools_for("sam");
    assert!(
        !tool_call(&sam, "read_file", json!({ "path": "team/wiki/a.md" }))
            .await
            .is_error
    );
    f.state
        .checkpoints
        .undo_checkpoint(
            RepoKind::Team,
            second,
            CheckpointContext::system(CheckpointTrigger::Undo, "undo"),
            &crate::workspace::team_files::TeamWriter::Agent("robin".to_string()),
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
