//! Team layer bootstrapping: the shared `team/` directory every agent on the
//! hub reads from, and each agent's role page in the team wiki.

use std::fmt::Write as _;
use std::path::Path;

use tokio::io::AsyncWriteExt;

use crate::config::paths::TeamPaths;
use crate::util::FatalError;
use crate::workspace::team_files::{TeamPathGuard, TeamWriteCoordinator, TeamWriter};

const DEFAULT_AGENTS: &str = include_str!("../../assets/team-bootstrap/AGENTS.md");
const DEFAULT_USER: &str = include_str!("../../assets/team-bootstrap/USER.md");
const DEFAULT_WIKI_INDEX: &str = include_str!("../../assets/team-bootstrap/wiki/index.md");
const DEFAULT_WIKI_LOG: &str = include_str!("../../assets/team-bootstrap/wiki/log.md");
const DEFAULT_WIKI_AGENTS_INDEX: &str =
    include_str!("../../assets/team-bootstrap/wiki/agents/index.md");

/// The first agent's `HEARTBEAT.yml`: the built-in pulses including
/// `wiki_lint`, which checks the shared team wiki and so belongs to exactly
/// one agent.
const FIRST_AGENT_HEARTBEAT: &str = concat!(
    include_str!("../../assets/workspace-bootstrap/heartbeat/base.yml"),
    include_str!("../../assets/workspace-bootstrap/heartbeat/wiki-lint.yml"),
    include_str!("../../assets/workspace-bootstrap/heartbeat/starters.yml"),
);

/// The created-agent `HEARTBEAT.yml`: the same built-in pulses without
/// `wiki_lint`.
const CREATED_AGENT_HEARTBEAT: &str = concat!(
    include_str!("../../assets/workspace-bootstrap/heartbeat/base.yml"),
    include_str!("../../assets/workspace-bootstrap/heartbeat/starters.yml"),
);

/// Description written to a role page whose agent has not described its
/// role yet.
const ROLE_PLACEHOLDER: &str = "Role not described yet; this agent fills it in.";

/// The `HEARTBEAT.yml` for the first agent (onboarding): the built-in
/// pulses, including `wiki_lint`.
#[must_use]
pub fn first_agent_heartbeat() -> &'static str {
    FIRST_AGENT_HEARTBEAT
}

/// The `HEARTBEAT.yml` template for created agents: the first agent's
/// built-in pulses without `wiki_lint`, so exactly one agent lints the
/// shared wiki. `memory_tending` stays, since it files each agent's own
/// memory into the wiki.
#[must_use]
pub fn created_agent_heartbeat() -> &'static str {
    CREATED_AGENT_HEARTBEAT
}

/// Create the team directory tree and write the team defaults that are
/// missing: `AGENTS.md`, `USER.md`, the wiki index and log, and the agent
/// roster index. Existing files are never modified.
///
/// When `user_name` is provided and `USER.md` does not yet exist, the
/// default content is personalised with the name; `timezone` is added the
/// same way.
///
/// # Errors
/// Returns `FatalError::Workspace` if a directory cannot be created or a
/// default file cannot be written.
#[tracing::instrument(skip_all, fields(team = %team.root().display()))]
pub async fn ensure_team(
    team: &TeamPaths,
    user_name: Option<&str>,
    timezone: Option<&str>,
) -> Result<(), FatalError> {
    for dir in team.required_dirs() {
        tokio::fs::create_dir_all(&dir).await.map_err(|e| {
            FatalError::Workspace(format!("failed to create directory {}: {e}", dir.display()))
        })?;
    }

    write_if_missing(&team.agents_md(), DEFAULT_AGENTS).await?;
    write_if_missing(&team.user_md(), &build_user_content(user_name, timezone)).await?;
    write_if_missing(&team.wiki_index_md(), DEFAULT_WIKI_INDEX).await?;
    write_if_missing(&team.wiki_log_md(), DEFAULT_WIKI_LOG).await?;
    write_if_missing(&team.wiki_agents_index_md(), DEFAULT_WIKI_AGENTS_INDEX).await?;
    Ok(())
}

/// Create the agent's role page `team/wiki/agents/<name>.md`, add its entry
/// to `team/wiki/agents/index.md`, and append a line to `team/wiki/log.md`.
///
/// `description` is the one-line role; without one the page carries a
/// placeholder the agent replaces itself. Does nothing when the page
/// already exists, since the agent maintains it. Returns whether the page
/// was created.
///
/// # Errors
/// Returns `FatalError::Workspace` if the page, the index, or the log
/// cannot be written.
///
/// The page, the roster index, and the log are written under `coordinator`'s
/// path locks, with the agent recorded as their writer, so agents starting
/// at the same time neither lose each other's entries nor bypass the team
/// write checks. `coordinator` must guard `team`.
pub async fn ensure_agent_role_page(
    team: &TeamPaths,
    coordinator: &TeamWriteCoordinator,
    name: &str,
    description: Option<&str>,
) -> Result<bool, FatalError> {
    let page_path = team.agent_role_page(name);
    let index_path = team.wiki_agents_index_md();
    let log_path = team.wiki_log_md();
    let writer = TeamWriter::Agent(name.to_string());
    let guards = coordinator
        .lock_all(&[page_path.clone(), index_path.clone(), log_path.clone()])
        .await;
    let guard_for = |path: &Path| {
        guards
            .iter()
            .find(|guard| guard.path() == path)
            .ok_or_else(|| {
                FatalError::Workspace(format!("no write lock held for {}", path.display()))
            })
    };
    let page_guard = guard_for(&page_path)?;
    let index_guard = guard_for(&index_path)?;
    let log_guard = guard_for(&log_path)?;

    if exists(&page_path).await? {
        return Ok(false);
    }

    tokio::fs::create_dir_all(team.wiki_agents_dir())
        .await
        .map_err(|e| {
            FatalError::Workspace(format!(
                "failed to create directory {}: {e}",
                team.wiki_agents_dir().display()
            ))
        })?;

    let role = description
        .map(single_line)
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| ROLE_PLACEHOLDER.to_string());

    // Index entry first, page second: a crash between the two leaves an
    // index entry that the retry skips (it checks for the link) before
    // writing the page.
    add_index_entry(index_guard, &writer, name, &role).await?;
    page_guard
        .commit(&writer, role_page_content(name, &role).as_bytes())
        .await
        .map_err(|e| {
            FatalError::Workspace(format!("failed to write {}: {e:#}", page_path.display()))
        })?;
    append_log_line(log_guard, &writer, name).await?;

    tracing::info!(agent = %name, page = %page_path.display(), "wrote agent role page");
    Ok(true)
}

fn role_page_content(name: &str, role: &str) -> String {
    format!(
        "---\ntype: Agent\ntitle: {title}\ndescription: {description}\n---\n\n# {name}\n\n\
         {name} maintains this page with its role and responsibilities: what it is for, \
         what it owns, and what teammates should hand it.\n",
        title = yaml_scalar(name),
        description = yaml_quoted(role),
    )
}

/// Append `- [name](/agents/name.md) — role` to the roster index unless a
/// link to the page is already there.
async fn add_index_entry(
    index: &TeamPathGuard,
    writer: &TeamWriter,
    name: &str,
    role: &str,
) -> Result<(), FatalError> {
    let index_path = index.path().to_path_buf();
    let existing = match tokio::fs::read_to_string(&index_path).await {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => DEFAULT_WIKI_AGENTS_INDEX.to_string(),
        Err(e) => {
            return Err(FatalError::Workspace(format!(
                "failed to read agent roster index {}: {e}",
                index_path.display()
            )));
        }
    };

    let link = format!("(/agents/{name}.md)");
    if existing.contains(&link) {
        return Ok(());
    }

    let mut updated = existing;
    if !updated.ends_with('\n') {
        updated.push('\n');
    }
    _ = writeln!(updated, "- [{name}]{link} — {role}");
    index
        .commit(writer, updated.as_bytes())
        .await
        .map(drop)
        .map_err(|e| {
            FatalError::Workspace(format!(
                "failed to update agent roster index {}: {e:#}",
                index_path.display()
            ))
        })
}

/// Append the role page's `edit` entry to the wiki log.
async fn append_log_line(
    log: &TeamPathGuard,
    writer: &TeamWriter,
    name: &str,
) -> Result<(), FatalError> {
    let log_path = log.path().to_path_buf();
    let date = chrono::Local::now().format("%Y-%m-%d");
    let line = format!("## [{date}] edit | added role page agents/{name}.md for agent {name}\n");

    let mut existing = match tokio::fs::read_to_string(&log_path).await {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => DEFAULT_WIKI_LOG.to_string(),
        Err(e) => {
            return Err(FatalError::Workspace(format!(
                "failed to read wiki log {}: {e}",
                log_path.display()
            )));
        }
    };
    if !existing.ends_with('\n') {
        existing.push('\n');
    }
    existing.push_str(&line);
    log.commit(writer, existing.as_bytes())
        .await
        .map(drop)
        .map_err(|e| {
            FatalError::Workspace(format!(
                "failed to update wiki log {}: {e:#}",
                log_path.display()
            ))
        })
}

/// Build USER.md content from optional name and timezone.
fn build_user_content(user_name: Option<&str>, timezone: Option<&str>) -> String {
    let name = user_name.filter(|n| !n.is_empty());
    let tz = timezone.filter(|t| !t.is_empty());

    let mut out = DEFAULT_USER.to_string();
    if name.is_none() && tz.is_none() {
        return out;
    }
    if let Some(name) = name {
        out.push_str("\n**Name**: ");
        out.push_str(name);
    }
    if let Some(tz) = tz {
        out.push_str("\n**Timezone**: ");
        out.push_str(tz);
    }
    out.push('\n');
    out
}

/// Collapse whitespace (including newlines) so a description stays on one
/// line in frontmatter and in the index.
fn single_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A YAML double-quoted scalar. JSON string syntax is valid YAML, and it
/// escapes everything that could break the frontmatter.
fn yaml_quoted(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string())
}

/// `name` as a YAML scalar: plain when it can only be read as a string,
/// quoted when YAML would read it as a number, boolean, or null.
fn yaml_scalar(name: &str) -> String {
    let reads_as_string = name.starts_with(|c: char| c.is_ascii_lowercase())
        && !matches!(
            name,
            "true" | "false" | "null" | "yes" | "no" | "on" | "off" | "y" | "n"
        );
    if reads_as_string {
        name.to_string()
    } else {
        yaml_quoted(name)
    }
}

async fn exists(path: &Path) -> Result<bool, FatalError> {
    tokio::fs::try_exists(path)
        .await
        .map_err(|e| FatalError::Workspace(format!("failed to check {}: {e}", path.display())))
}

/// Write `content` to `path` if it does not exist yet.
///
/// Agents starting at the same time each run this against a fresh team
/// layer, so losing the race to create the file is not an error: the winner's
/// copy is the file.
async fn write_if_missing(path: &Path, content: &str) -> Result<(), FatalError> {
    if !exists(path).await? && write_new(path, content).await? {
        tracing::debug!(path = %path.display(), "wrote team default");
    }
    Ok(())
}

/// Create `path` and write `content`. Returns `false`, leaving the file
/// alone, when it already exists.
async fn write_new(path: &Path, content: &str) -> Result<bool, FatalError> {
    let write_error = |e: std::io::Error| {
        FatalError::Workspace(format!("failed to write {}: {e}", path.display()))
    };
    let mut file = match tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .await
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
        Err(e) => return Err(write_error(e)),
    };
    file.write_all(content.as_bytes())
        .await
        .map_err(write_error)?;
    file.flush().await.map_err(write_error)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pulse::types::HeartbeatConfig;

    fn team_in(dir: &Path) -> TeamPaths {
        TeamPaths::new(dir.join("team"))
    }

    #[tokio::test]
    async fn ensure_team_writes_defaults_and_directories() {
        let dir = tempfile::tempdir().unwrap();
        let team = team_in(dir.path());
        ensure_team(&team, None, None).await.unwrap();

        for required in team.required_dirs() {
            assert!(required.is_dir(), "{} should exist", required.display());
        }
        for file in [
            team.agents_md(),
            team.user_md(),
            team.wiki_index_md(),
            team.wiki_log_md(),
            team.wiki_agents_index_md(),
        ] {
            assert!(file.is_file(), "{} should exist", file.display());
        }
        let index = std::fs::read_to_string(team.wiki_index_md()).unwrap();
        assert!(index.contains("(/agents/index.md)"));
    }

    #[tokio::test]
    async fn write_new_leaves_a_file_someone_else_created_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("AGENTS.md");
        std::fs::write(&path, "the winner's copy").unwrap();

        let created = write_new(&path, "my default").await.unwrap();

        assert!(!created, "losing the creation race is not an error");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "the winner's copy");
    }

    #[tokio::test]
    async fn agents_starting_together_on_a_fresh_team_layer_all_succeed() {
        let dir = tempfile::tempdir().unwrap();
        let team = team_in(dir.path());

        let starts = (0..8).map(|_| ensure_team(&team, None, None));
        let results = futures_util::future::join_all(starts).await;

        for result in results {
            result.unwrap();
        }
        assert!(team.agents_md().is_file());
    }

    #[tokio::test]
    async fn ensure_team_never_overwrites_existing_files() {
        let dir = tempfile::tempdir().unwrap();
        let team = team_in(dir.path());
        ensure_team(&team, None, None).await.unwrap();
        std::fs::write(team.agents_md(), "my rules").unwrap();
        std::fs::write(team.user_md(), "my user").unwrap();
        std::fs::write(team.wiki_index_md(), "my index").unwrap();

        ensure_team(&team, Some("Bear"), Some("UTC")).await.unwrap();

        assert_eq!(
            std::fs::read_to_string(team.agents_md()).unwrap(),
            "my rules"
        );
        assert_eq!(std::fs::read_to_string(team.user_md()).unwrap(), "my user");
        assert_eq!(
            std::fs::read_to_string(team.wiki_index_md()).unwrap(),
            "my index"
        );
    }

    #[tokio::test]
    async fn ensure_team_personalizes_a_new_user_md() {
        let dir = tempfile::tempdir().unwrap();
        let team = team_in(dir.path());
        ensure_team(&team, Some("Bear"), Some("America/Chicago"))
            .await
            .unwrap();
        let user = std::fs::read_to_string(team.user_md()).unwrap();
        assert!(user.contains("**Name**: Bear"));
        assert!(user.contains("**Timezone**: America/Chicago"));
    }

    #[tokio::test]
    async fn role_page_has_frontmatter_index_entry_and_log_line() {
        let dir = tempfile::tempdir().unwrap();
        let team = team_in(dir.path());
        ensure_team(&team, None, None).await.unwrap();

        let created =
            ensure_agent_role_page(&team, &TeamWriteCoordinator::new(&team), "scout", None)
                .await
                .unwrap();
        assert!(created);

        let page = std::fs::read_to_string(team.agent_role_page("scout")).unwrap();
        assert!(page.starts_with("---\ntype: Agent\ntitle: scout\ndescription: \""));
        assert!(page.contains("scout maintains this page"));
        let index = std::fs::read_to_string(team.wiki_agents_index_md()).unwrap();
        assert!(index.contains("- [scout](/agents/scout.md) — "));
        let log = std::fs::read_to_string(team.wiki_log_md()).unwrap();
        assert!(log.contains("] edit | added role page agents/scout.md"));
    }

    #[tokio::test]
    async fn role_page_uses_the_description_on_one_line() {
        let dir = tempfile::tempdir().unwrap();
        let team = team_in(dir.path());
        ensure_agent_role_page(
            &team,
            &TeamWriteCoordinator::new(&team),
            "scout",
            Some("Watches feeds:\nreports \"news\""),
        )
        .await
        .unwrap();

        let page = std::fs::read_to_string(team.agent_role_page("scout")).unwrap();
        let front: serde_yaml_ng::Value = serde_yaml_ng::from_str(
            page.trim_start_matches("---\n")
                .split("\n---\n")
                .next()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(front.get("type").and_then(|v| v.as_str()), Some("Agent"));
        assert_eq!(front.get("title").and_then(|v| v.as_str()), Some("scout"));
        assert_eq!(
            front.get("description").and_then(|v| v.as_str()),
            Some("Watches feeds: reports \"news\"")
        );
        let index = std::fs::read_to_string(team.wiki_agents_index_md()).unwrap();
        assert!(index.contains("— Watches feeds: reports \"news\""));
    }

    #[tokio::test]
    async fn role_page_creation_is_idempotent_and_keeps_agent_edits() {
        let dir = tempfile::tempdir().unwrap();
        let team = team_in(dir.path());
        assert!(
            ensure_agent_role_page(&team, &TeamWriteCoordinator::new(&team), "scout", None)
                .await
                .unwrap()
        );
        std::fs::write(team.agent_role_page("scout"), "agent's own words").unwrap();
        let index = std::fs::read_to_string(team.wiki_agents_index_md()).unwrap();
        let log = std::fs::read_to_string(team.wiki_log_md()).unwrap();

        assert!(
            !ensure_agent_role_page(
                &team,
                &TeamWriteCoordinator::new(&team),
                "scout",
                Some("other")
            )
            .await
            .unwrap()
        );

        assert_eq!(
            std::fs::read_to_string(team.agent_role_page("scout")).unwrap(),
            "agent's own words"
        );
        assert_eq!(
            std::fs::read_to_string(team.wiki_agents_index_md()).unwrap(),
            index
        );
        assert_eq!(std::fs::read_to_string(team.wiki_log_md()).unwrap(), log);
    }

    #[tokio::test]
    async fn role_pages_for_several_agents_share_one_index() {
        let dir = tempfile::tempdir().unwrap();
        let team = team_in(dir.path());
        ensure_agent_role_page(&team, &TeamWriteCoordinator::new(&team), "alpha", None)
            .await
            .unwrap();
        ensure_agent_role_page(
            &team,
            &TeamWriteCoordinator::new(&team),
            "beta",
            Some("Second"),
        )
        .await
        .unwrap();

        let index = std::fs::read_to_string(team.wiki_agents_index_md()).unwrap();
        assert_eq!(index.matches("(/agents/alpha.md)").count(), 1);
        assert_eq!(index.matches("(/agents/beta.md)").count(), 1);
    }

    #[tokio::test]
    async fn a_retry_after_a_crash_between_index_and_page_adds_no_duplicate() {
        let dir = tempfile::tempdir().unwrap();
        let team = team_in(dir.path());
        ensure_agent_role_page(&team, &TeamWriteCoordinator::new(&team), "scout", None)
            .await
            .unwrap();
        std::fs::remove_file(team.agent_role_page("scout")).unwrap();

        assert!(
            ensure_agent_role_page(&team, &TeamWriteCoordinator::new(&team), "scout", None)
                .await
                .unwrap()
        );

        let index = std::fs::read_to_string(team.wiki_agents_index_md()).unwrap();
        assert_eq!(index.matches("(/agents/scout.md)").count(), 1);
    }

    #[test]
    fn numeric_and_boolean_names_are_quoted_in_frontmatter() {
        assert_eq!(yaml_scalar("scout"), "scout");
        assert_eq!(yaml_scalar("123"), "\"123\"");
        assert_eq!(yaml_scalar("null"), "\"null\"");
        assert_eq!(yaml_scalar("-x"), "\"-x\"");
    }

    fn pulse_names(yaml: &str) -> Vec<String> {
        let cfg: HeartbeatConfig = serde_yaml_ng::from_str(yaml).unwrap();
        cfg.pulses.into_iter().map(|p| p.name).collect()
    }

    #[test]
    fn first_agent_heartbeat_includes_wiki_lint() {
        assert_eq!(
            pulse_names(first_agent_heartbeat()),
            ["reflection", "memory_tending", "wiki_lint"]
        );
    }

    #[test]
    fn created_agent_heartbeat_keeps_memory_tending_without_wiki_lint() {
        let template = created_agent_heartbeat();
        assert_eq!(pulse_names(template), ["reflection", "memory_tending"]);
        assert!(!template.contains("wiki_lint"));
        assert!(template.contains("# ── Starter pulses"));
    }
}
