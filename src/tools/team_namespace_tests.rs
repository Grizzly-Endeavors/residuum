//! The `team/` namespace and team write coordination, exercised through the
//! same registry the agent's file tools are registered in.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{FileTracker, PathPolicy, ToolRegistry, ToolResult};
use crate::diagnostics::DiagnosticsPaths;
use crate::workspace::team_files::{TeamWriteCoordinator, TeamWriter};

/// A hub directory with a team directory and any number of agent directories.
pub(crate) struct Hub {
    pub(crate) dir: tempfile::TempDir,
    pub(crate) coordinator: TeamWriteCoordinator,
}

impl Hub {
    pub(crate) fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("team")).unwrap();
        let coordinator = TeamWriteCoordinator::new(dir.path().join("team"));
        Self { dir, coordinator }
    }

    pub(crate) fn team(&self) -> PathBuf {
        self.dir.path().join("team")
    }

    pub(crate) fn agent_dir(&self, name: &str) -> PathBuf {
        let dir = self.dir.path().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A fresh tool set for the agent `name`: its own read tracker, sharing
    /// the hub's coordinator.
    pub(crate) fn tools_for(&self, name: &str) -> ToolRegistry {
        let agent_dir = self.agent_dir(name);
        let policy = PathPolicy::new_shared_with_team(
            HashSet::new(),
            self.coordinator.view_for_agent(name, &agent_dir),
        );
        let mut registry = ToolRegistry::new();
        registry.register_defaults(
            FileTracker::new_shared(),
            policy,
            DiagnosticsPaths {
                config_dir: agent_dir.join("config"),
                workspace_dir: agent_dir,
                hub_dir: self.dir.path().join("hub"),
            },
            None,
        );
        registry
    }
}

async fn call(tools: &ToolRegistry, name: &str, args: Value) -> ToolResult {
    tools.execute(name, args).await.unwrap()
}

async fn read(tools: &ToolRegistry, path: &str) -> ToolResult {
    call(tools, "read_file", json!({ "path": path })).await
}

async fn write(tools: &ToolRegistry, path: &str, content: &str) -> ToolResult {
    call(
        tools,
        "write_file",
        json!({ "path": path, "content": content }),
    )
    .await
}

async fn edit(tools: &ToolRegistry, path: &str, old: &str, new: &str) -> ToolResult {
    call(
        tools,
        "edit_file",
        json!({ "path": path, "edits": [{ "old_string": old, "new_string": new }] }),
    )
    .await
}

fn path_str(path: &Path) -> &str {
    path.to_str().unwrap()
}

#[tokio::test]
async fn read_resolves_team_paths_into_the_team_directory() {
    let hub = Hub::new();
    std::fs::create_dir_all(hub.team().join("wiki")).unwrap();
    std::fs::write(hub.team().join("wiki").join("a.md"), "shared page").unwrap();
    std::fs::write(hub.agent_dir("scout").join("notes.md"), "private notes").unwrap();
    let tools = hub.tools_for("scout");

    let shared = read(&tools, "team/wiki/a.md").await;
    assert!(!shared.is_error, "{}", shared.output);
    assert!(shared.output.contains("shared page"));

    let private = read(&tools, "notes.md").await;
    assert!(!private.is_error, "{}", private.output);
    assert!(private.output.contains("private notes"));
}

#[tokio::test]
async fn write_and_edit_resolve_team_paths_into_the_team_directory() {
    let hub = Hub::new();
    let tools = hub.tools_for("scout");

    let created = write(&tools, "team/wiki/new.md", "one two").await;
    assert!(!created.is_error, "{}", created.output);
    let on_disk = hub.team().join("wiki").join("new.md");
    assert_eq!(std::fs::read_to_string(&on_disk).unwrap(), "one two");
    assert!(
        !hub.agent_dir("scout").join("team").exists(),
        "the agent directory must not gain a team entry"
    );

    let edited = edit(&tools, "team/wiki/new.md", "two", "three").await;
    assert!(!edited.is_error, "{}", edited.output);
    assert_eq!(std::fs::read_to_string(&on_disk).unwrap(), "one three");
}

#[tokio::test]
async fn absolute_team_paths_keep_working() {
    let hub = Hub::new();
    let tools = hub.tools_for("scout");
    let absolute = hub.team().join("abs.md");

    let created = write(&tools, path_str(&absolute), "absolute").await;
    assert!(!created.is_error, "{}", created.output);
    let read_back = read(&tools, "team/abs.md").await;
    assert!(
        read_back.output.contains("absolute"),
        "{}",
        read_back.output
    );
}

#[tokio::test]
async fn the_agent_directory_cannot_gain_a_team_entry() {
    let hub = Hub::new();
    let tools = hub.tools_for("scout");
    let inside = hub.agent_dir("scout").join("team").join("x.md");

    let created = write(&tools, path_str(&inside), "nope").await;
    assert!(created.is_error);
    assert!(
        created.output.contains("team"),
        "the error should explain the reservation: {}",
        created.output
    );
    let edited = edit(&tools, path_str(&inside), "a", "b").await;
    assert!(edited.is_error);
    assert!(edited.output.contains("team"), "{}", edited.output);
    assert!(!hub.agent_dir("scout").join("team").exists());
}

#[tokio::test]
async fn a_stale_write_is_refused_and_names_the_teammate() {
    let hub = Hub::new();
    std::fs::write(hub.team().join("shared.md"), "start").unwrap();
    let robin = hub.tools_for("robin");
    let sam = hub.tools_for("sam");
    assert!(!read(&robin, "team/shared.md").await.is_error);
    assert!(!read(&sam, "team/shared.md").await.is_error);

    let first = write(&robin, "team/shared.md", "robin was here").await;
    assert!(!first.is_error, "{}", first.output);

    let second = write(&sam, "team/shared.md", "sam was here").await;
    assert!(second.is_error);
    assert!(
        second.output.contains("team/shared.md"),
        "{}",
        second.output
    );
    assert!(
        second.output.contains("teammate robin"),
        "{}",
        second.output
    );
    assert!(second.output.contains("read_file"), "{}", second.output);
    assert_eq!(
        std::fs::read_to_string(hub.team().join("shared.md")).unwrap(),
        "robin was here",
        "the refused write must not touch the file"
    );

    // Reading again brings sam up to date, so his next write goes through.
    assert!(!read(&sam, "team/shared.md").await.is_error);
    let retry = write(&sam, "team/shared.md", "sam was here").await;
    assert!(!retry.is_error, "{}", retry.output);
}

#[tokio::test]
async fn a_stale_edit_is_refused_and_names_the_teammate() {
    let hub = Hub::new();
    std::fs::write(hub.team().join("shared.md"), "alpha beta").unwrap();
    let robin = hub.tools_for("robin");
    let sam = hub.tools_for("sam");
    assert!(!read(&robin, "team/shared.md").await.is_error);
    assert!(!read(&sam, "team/shared.md").await.is_error);

    let first = edit(&robin, "team/shared.md", "alpha", "ALPHA").await;
    assert!(!first.is_error, "{}", first.output);
    let second = edit(&sam, "team/shared.md", "beta", "BETA").await;
    assert!(second.is_error);
    assert!(
        second.output.contains("teammate robin"),
        "{}",
        second.output
    );
    assert_eq!(
        std::fs::read_to_string(hub.team().join("shared.md")).unwrap(),
        "ALPHA beta"
    );
}

#[tokio::test]
async fn concurrent_writers_leave_exactly_one_winner() {
    let hub = Hub::new();
    std::fs::write(hub.team().join("shared.md"), "start").unwrap();
    let robin = hub.tools_for("robin");
    let sam = hub.tools_for("sam");
    assert!(!read(&robin, "team/shared.md").await.is_error);
    assert!(!read(&sam, "team/shared.md").await.is_error);

    let (from_robin, from_sam) = tokio::join!(
        write(&robin, "team/shared.md", "robin"),
        write(&sam, "team/shared.md", "sam"),
    );

    let (winner, loser, loser_result) = match (from_robin.is_error, from_sam.is_error) {
        (false, true) => ("robin", "sam", from_sam),
        (true, false) => ("sam", "robin", from_robin),
        other => panic!("exactly one write should succeed, got errors {other:?}"),
    };
    assert!(
        loser_result.output.contains(&format!("teammate {winner}")),
        "{loser} should be told {winner} changed the file: {}",
        loser_result.output
    );
    assert_eq!(
        std::fs::read_to_string(hub.team().join("shared.md")).unwrap(),
        winner
    );
}

#[tokio::test]
async fn a_web_write_is_named_as_the_user() {
    let hub = Hub::new();
    std::fs::write(hub.team().join("shared.md"), "start").unwrap();
    let sam = hub.tools_for("sam");
    assert!(!read(&sam, "team/shared.md").await.is_error);

    let path = hub.team().join("shared.md");
    let guard = hub.coordinator.lock(&path).await;
    guard
        .commit(&TeamWriter::User, b"edited in the browser")
        .await
        .unwrap();
    drop(guard);

    let refused = write(&sam, "team/shared.md", "sam was here").await;
    assert!(refused.is_error);
    assert!(refused.output.contains("the user"), "{}", refused.output);
}

#[tokio::test]
async fn an_outside_change_is_an_unknown_writer() {
    let hub = Hub::new();
    std::fs::write(hub.team().join("shared.md"), "start").unwrap();
    let sam = hub.tools_for("sam");
    assert!(!read(&sam, "team/shared.md").await.is_error);

    std::fs::write(hub.team().join("shared.md"), "changed by an editor").unwrap();

    let refused = write(&sam, "team/shared.md", "sam was here").await;
    assert!(refused.is_error);
    assert!(
        refused.output.contains("unknown writer"),
        "{}",
        refused.output
    );
    let refused_edit = edit(&sam, "team/shared.md", "editor", "person").await;
    assert!(refused_edit.is_error);
    assert!(refused_edit.output.contains("unknown writer"));
}

#[tokio::test]
async fn two_agents_creating_the_same_file_leave_one_winner() {
    let hub = Hub::new();
    let robin = hub.tools_for("robin");
    let sam = hub.tools_for("sam");

    let (from_robin, from_sam) = tokio::join!(
        write(&robin, "team/new.md", "robin"),
        write(&sam, "team/new.md", "sam"),
    );

    let (winner, loser_result) = match (from_robin.is_error, from_sam.is_error) {
        (false, true) => ("robin", from_sam),
        (true, false) => ("sam", from_robin),
        other => panic!("exactly one create should succeed, got errors {other:?}"),
    };
    assert!(
        loser_result.output.contains(&format!("teammate {winner}")),
        "{}",
        loser_result.output
    );
    assert_eq!(
        std::fs::read_to_string(hub.team().join("new.md")).unwrap(),
        winner
    );
}

#[tokio::test]
async fn an_agents_own_writes_do_not_conflict_with_each_other() {
    let hub = Hub::new();
    let sam = hub.tools_for("sam");
    assert!(!write(&sam, "team/mine.md", "one").await.is_error);
    assert!(!write(&sam, "team/mine.md", "two!").await.is_error);
    assert!(!edit(&sam, "team/mine.md", "two", "three").await.is_error);
    assert_eq!(
        std::fs::read_to_string(hub.team().join("mine.md")).unwrap(),
        "three!"
    );
}

#[tokio::test]
async fn agent_private_files_keep_todays_behavior() {
    let hub = Hub::new();
    let private = hub.agent_dir("sam").join("notes.md");
    std::fs::write(&private, "start").unwrap();
    let sam = hub.tools_for("sam");
    assert!(!read(&sam, "notes.md").await.is_error);

    // Changed behind the agent's back: a private file is still overwritten,
    // exactly as before team coordination existed.
    std::fs::write(&private, "changed elsewhere").unwrap();
    let written = write(&sam, "notes.md", "sam's version").await;
    assert!(!written.is_error, "{}", written.output);
    assert_eq!(std::fs::read_to_string(&private).unwrap(), "sam's version");

    // Read-before-overwrite still applies.
    let unread = hub.agent_dir("sam").join("unread.md");
    std::fs::write(&unread, "x").unwrap();
    let refused = write(&sam, "unread.md", "y").await;
    assert!(refused.is_error);
    assert!(refused.output.contains("has not been read"));
}

#[tokio::test]
async fn team_files_get_the_same_diagnostics_as_agent_files() {
    let hub = Hub::new();
    let tools = hub.tools_for("scout");

    let result = write(&tools, "team/skills/broken/SKILL.md", "no frontmatter here").await;
    assert!(!result.is_error, "{}", result.output);
    assert!(
        result.output.contains("SKILL.md") && result.output.lines().count() > 1,
        "a malformed team skill should be reported: {}",
        result.output
    );
}
