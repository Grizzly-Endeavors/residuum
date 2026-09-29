//! The `team/` namespace in the workspace file API, and write coordination
//! between the API and agents' file tools.

use std::path::PathBuf;

use axum::extract::{Query, RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Json, Response};
use serde_json::{Value, json};

use super::ConfigApiState;
use super::workspace::{
    DeleteFileQuery, FileQuery, FilesQuery, MkdirRequest, MoveRequest, WriteFileRequest,
    api_workspace_delete, api_workspace_file_read, api_workspace_file_write, api_workspace_files,
    api_workspace_mkdir, api_workspace_move, api_workspace_raw_read,
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
    let state = ConfigApiState {
        team: Some(hub.coordinator.view_for_user(&agent_dir)),
        hub_dir: hub.dir.path().join("hub"),
        config_dir: agent_dir.join("config"),
        agent_name: "scout".to_string(),
        workspace_dir: agent_dir.clone(),
        memory_dir: None,
        reload_tx: None,
        setup_done: None,
        secret_lock: std::sync::Arc::new(tokio::sync::Mutex::new(())),
        checkpoints: crate::checkpoints::test_engine(),
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
