//! Workspace bootstrapping: creates required directories and default identity files.

use crate::util::FatalError;

use super::layout::WorkspaceLayout;

// ── Workspace bootstrap content (embedded at compile time from assets/) ──────

const DEFAULT_SOUL: &str = include_str!("../../assets/workspace-bootstrap/SOUL.md");
const DEFAULT_AGENTS: &str = include_str!("../../assets/workspace-bootstrap/AGENTS.md");
const DEFAULT_USER: &str = include_str!("../../assets/workspace-bootstrap/USER.md");
const DEFAULT_WIKI_INDEX: &str = include_str!("../../assets/workspace-bootstrap/wiki/index.md");
const DEFAULT_WIKI_LOG: &str = include_str!("../../assets/workspace-bootstrap/wiki/log.md");

/// Default content for BOOTSTRAP.md -- first-run guidance.
///
/// This file is written once during workspace creation and should be deleted
/// by the agent after the first conversation. A `.bootstrapped` sentinel file
/// prevents it from being recreated on subsequent startups.
const DEFAULT_BOOTSTRAP: &str = include_str!("../../assets/workspace-bootstrap/BOOTSTRAP.md");

/// Default observer content guidance written to memory/OBSERVER.md.
///
/// Contains only the customizable content portion — the output format spec is
/// always injected by the Rust code and cannot be lost by editing this file.
const DEFAULT_OBSERVER_PROMPT: &str =
    include_str!("../../assets/workspace-bootstrap/memory/OBSERVER.md");

/// Default reflector content guidance written to memory/REFLECTOR.md.
///
/// Contains only the customizable content portion — the output format spec is
/// always injected by the Rust code and cannot be lost by editing this file.
const DEFAULT_REFLECTOR_PROMPT: &str =
    include_str!("../../assets/workspace-bootstrap/memory/REFLECTOR.md");

const DEFAULT_HEARTBEAT: &str = include_str!("../../assets/workspace-bootstrap/HEARTBEAT.yml");

/// Built-in `introspection` skill, used by the reflection pulse to review
/// episode memory and deliver suggestions.
const INTROSPECTION_SKILL_MD: &str =
    include_str!("../../assets/bundled-skills/introspection/SKILL.md");

/// Built-in `learner` skill, spawned by the subconscious when a single learnable
/// signal is detected in the live conversation. Corroborates the signal and makes
/// it durable (preference promotion or a queued recovery fix).
const LEARNER_SKILL_MD: &str = include_str!("../../assets/bundled-skills/learner/SKILL.md");

/// Built-in `memory-analyst` skill, used to answer synthesized questions about the
/// user or past history so the main agent gets grounded conclusions instead of raw
/// search excerpts.
const MEMORY_ANALYST_SKILL_MD: &str =
    include_str!("../../assets/bundled-skills/memory-analyst/SKILL.md");

/// Built-in `wiki` skill: the knowledge wiki's page format, index rules, and
/// ingest/lint procedures. Activated before writing to the wiki, and the role
/// of the `memory_tending` and `wiki_lint` pulses.
const WIKI_SKILL_MD: &str = include_str!("../../assets/bundled-skills/wiki/SKILL.md");

/// Built-in `workbench` skill: building interactive HTML tools in `workbench/`
/// that the web UI shows sandboxed, with the injected `residuum` SDK.
const WORKBENCH_SKILL_MD: &str = include_str!("../../assets/bundled-skills/workbench/SKILL.md");

/// Endpoint, event, and blocked-route reference for the `workbench` skill.
const WORKBENCH_REF_API: &str =
    include_str!("../../assets/bundled-skills/workbench/references/api.md");

/// Default subconscious check policy written to SUBCONSCIOUS.md.
///
/// Contains only the customizable check guidance — the output format spec is
/// always injected by the Rust code and cannot be lost by editing this file.
const DEFAULT_SUBCONSCIOUS: &str = include_str!("../../assets/workspace-bootstrap/SUBCONSCIOUS.md");

// ── Bundled skill content (embedded at compile time from assets/) ────────────

// residuum-system skill
const SYSTEM_SKILL_MD: &str = include_str!("../../assets/bundled-skills/residuum-system/SKILL.md");

/// Every reference file of the residuum-system skill, as `(file name, content)`.
/// SKILL.md links to each by name; the bootstrap writes all of them.
const SYSTEM_REFS: &[(&str, &str)] = &[
    (
        "agent-keys.md",
        include_str!("../../assets/bundled-skills/residuum-system/references/agent-keys.md"),
    ),
    (
        "memory-system.md",
        include_str!("../../assets/bundled-skills/residuum-system/references/memory-system.md"),
    ),
    (
        "heartbeats.md",
        include_str!("../../assets/bundled-skills/residuum-system/references/heartbeats.md"),
    ),
    (
        "inbox.md",
        include_str!("../../assets/bundled-skills/residuum-system/references/inbox.md"),
    ),
    (
        "scheduled-actions.md",
        include_str!("../../assets/bundled-skills/residuum-system/references/scheduled-actions.md"),
    ),
    (
        "skills.md",
        include_str!("../../assets/bundled-skills/residuum-system/references/skills.md"),
    ),
    (
        "tools.md",
        include_str!("../../assets/bundled-skills/residuum-system/references/tools.md"),
    ),
    (
        "mcp.md",
        include_str!("../../assets/bundled-skills/residuum-system/references/mcp.md"),
    ),
    (
        "notifications.md",
        include_str!("../../assets/bundled-skills/residuum-system/references/notifications.md"),
    ),
    (
        "background-tasks.md",
        include_str!("../../assets/bundled-skills/residuum-system/references/background-tasks.md"),
    ),
    (
        "subconscious.md",
        include_str!("../../assets/bundled-skills/residuum-system/references/subconscious.md"),
    ),
];

// residuum-getting-started skill
const GETTING_STARTED_SKILL_MD: &str =
    include_str!("../../assets/bundled-skills/residuum-getting-started/SKILL.md");
const GETTING_STARTED_ORGANIZED: &str = include_str!(
    "../../assets/bundled-skills/residuum-getting-started/workflows/getting-organized.md"
);
const GETTING_STARTED_MONITORING: &str = include_str!(
    "../../assets/bundled-skills/residuum-getting-started/workflows/monitoring-setup.md"
);
const GETTING_STARTED_EXTENDING: &str = include_str!(
    "../../assets/bundled-skills/residuum-getting-started/workflows/extending-capabilities.md"
);
const GETTING_STARTED_UNDERSTANDING: &str = include_str!(
    "../../assets/bundled-skills/residuum-getting-started/workflows/understanding-the-agent.md"
);
const GETTING_STARTED_ALWAYS_ON: &str = include_str!(
    "../../assets/bundled-skills/residuum-getting-started/workflows/always-on-assistant.md"
);

// skill-authoring skill
const SKILL_AUTHORING_SKILL_MD: &str =
    include_str!("../../assets/bundled-skills/skill-authoring/SKILL.md");
const SKILL_AUTHORING_REF_STANDARDS: &str =
    include_str!("../../assets/bundled-skills/skill-authoring/references/authoring-standards.md");

/// Ensure the workspace directory structure exists with default identity files.
///
/// When `user_name` is provided and `USER.md` does not yet exist, the default
/// content is personalised with the user's name. When `timezone` is provided,
/// it is included in `USER.md`.
///
/// This is idempotent: existing files and directories are not modified.
///
/// # Errors
/// Returns `FatalError::Workspace` if directories cannot be created or
/// default files cannot be written.
#[tracing::instrument(skip_all, fields(workspace = %layout.root().display()))]
pub async fn ensure_workspace(
    layout: &WorkspaceLayout,
    user_name: Option<&str>,
    timezone: Option<&str>,
) -> Result<(), FatalError> {
    // Create all required directories
    for dir in layout.required_dirs() {
        tokio::fs::create_dir_all(&dir).await.map_err(|e| {
            FatalError::Workspace(format!("failed to create directory {}: {e}", dir.display()))
        })?;
    }

    // Migration: Dual Inbox
    // Move any old flat `inbox/*.json` files to `inbox/agent/`
    let old_inbox = layout.root().join("inbox");
    if let Ok(mut entries) = tokio::fs::read_dir(&old_inbox).await {
        let agent_inbox = layout.agent_inbox_dir();
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.is_file()
                && path.extension().is_some_and(|e| e == "json")
                && let Some(name) = path.file_name()
            {
                let new_path = agent_inbox.join(name);
                if let Err(e) = tokio::fs::rename(&path, &new_path).await {
                    tracing::warn!(old = %path.display(), new = %new_path.display(), error = %e, "failed to migrate inbox item");
                } else {
                    tracing::info!(item = %name.to_string_lossy(), "migrated inbox item to agent inbox");
                }
            }
        }
    }

    // Create default identity files if they don't exist
    write_if_missing(&layout.soul_md(), DEFAULT_SOUL).await?;
    write_if_missing(&layout.agents_md(), DEFAULT_AGENTS).await?;

    let user_content = build_user_content(user_name, timezone);
    write_if_missing(&layout.user_md(), &user_content).await?;

    write_if_missing(&layout.wiki_index_md(), DEFAULT_WIKI_INDEX).await?;
    write_if_missing(&layout.wiki_log_md(), DEFAULT_WIKI_LOG).await?;

    // BOOTSTRAP.md is first-run only: write it once, then drop a sentinel so it
    // is never recreated after the agent deletes it.
    let sentinel = layout.root().join(".bootstrapped");
    let fresh_bootstrap = !tokio::fs::try_exists(&sentinel).await.map_err(|e| {
        FatalError::Workspace(format!(
            "failed to check bootstrap sentinel {}: {e}",
            sentinel.display()
        ))
    })?;
    if fresh_bootstrap {
        write_if_missing(&layout.bootstrap_md(), DEFAULT_BOOTSTRAP).await?;
        // Create the sentinel after writing BOOTSTRAP.md so that if we crash
        // between writing and sentinel creation, the next startup will retry.
        tokio::fs::write(&sentinel, "").await.map_err(|e| {
            FatalError::Workspace(format!(
                "failed to write bootstrap sentinel {}: {e}",
                sentinel.display()
            ))
        })?;
        tracing::debug!(sentinel = %sentinel.display(), "bootstrap sentinel written");
    }

    write_if_missing(&layout.observer_md(), DEFAULT_OBSERVER_PROMPT).await?;
    write_if_missing(&layout.reflector_md(), DEFAULT_REFLECTOR_PROMPT).await?;
    write_if_missing(&layout.heartbeat_yml(), DEFAULT_HEARTBEAT).await?;
    write_if_missing(&layout.subconscious_md(), DEFAULT_SUBCONSCIOUS).await?;

    // Write bundled skills
    write_bundled_skills(layout).await?;

    tracing::info!(
        workspace = %layout.root().display(),
        fresh_bootstrap,
        "workspace ready"
    );

    Ok(())
}

/// Build USER.md content from optional name and timezone.
fn build_user_content(user_name: Option<&str>, timezone: Option<&str>) -> String {
    let name = user_name.filter(|n| !n.is_empty());
    let tz = timezone.filter(|t| !t.is_empty());

    if name.is_none() && tz.is_none() {
        return DEFAULT_USER.to_string();
    }

    let mut out = DEFAULT_USER.to_string();
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

/// Write bundled skill trees to the workspace skills directory.
///
/// Each file is written with `write_if_missing`, so user edits are preserved
/// and files are only recreated if deleted.
async fn write_bundled_skills(layout: &WorkspaceLayout) -> Result<(), FatalError> {
    // residuum-system skill
    let system_dir = layout.skills_dir().join("residuum-system");
    let system_refs = system_dir.join("references");
    tokio::fs::create_dir_all(&system_refs).await.map_err(|e| {
        FatalError::Workspace(format!(
            "failed to create skill directory {}: {e}",
            system_refs.display()
        ))
    })?;

    write_if_missing(&system_dir.join("SKILL.md"), SYSTEM_SKILL_MD).await?;
    for (file_name, content) in SYSTEM_REFS {
        write_if_missing(&system_refs.join(file_name), content).await?;
    }

    // Single-file skills: role skills spawned as sub-agents by pulses, the
    // subconscious, and the main agent, plus the wiki conventions skill (also a
    // pulse role). None carries references, so each is a lone SKILL.md.
    for (name, body) in [
        ("introspection", INTROSPECTION_SKILL_MD),
        ("learner", LEARNER_SKILL_MD),
        ("memory-analyst", MEMORY_ANALYST_SKILL_MD),
        ("wiki", WIKI_SKILL_MD),
    ] {
        let dir = layout.skills_dir().join(name);
        tokio::fs::create_dir_all(&dir).await.map_err(|e| {
            FatalError::Workspace(format!(
                "failed to create skill directory {}: {e}",
                dir.display()
            ))
        })?;
        write_if_missing(&dir.join("SKILL.md"), body).await?;
    }

    // residuum-getting-started skill
    let started_dir = layout.skills_dir().join("residuum-getting-started");
    let started_workflows = started_dir.join("workflows");
    tokio::fs::create_dir_all(&started_workflows)
        .await
        .map_err(|e| {
            FatalError::Workspace(format!(
                "failed to create skill directory {}: {e}",
                started_workflows.display()
            ))
        })?;

    write_if_missing(&started_dir.join("SKILL.md"), GETTING_STARTED_SKILL_MD).await?;
    write_if_missing(
        &started_workflows.join("getting-organized.md"),
        GETTING_STARTED_ORGANIZED,
    )
    .await?;
    write_if_missing(
        &started_workflows.join("monitoring-setup.md"),
        GETTING_STARTED_MONITORING,
    )
    .await?;
    write_if_missing(
        &started_workflows.join("extending-capabilities.md"),
        GETTING_STARTED_EXTENDING,
    )
    .await?;
    write_if_missing(
        &started_workflows.join("understanding-the-agent.md"),
        GETTING_STARTED_UNDERSTANDING,
    )
    .await?;
    write_if_missing(
        &started_workflows.join("always-on-assistant.md"),
        GETTING_STARTED_ALWAYS_ON,
    )
    .await?;

    // skill-authoring skill
    let authoring_dir = layout.skills_dir().join("skill-authoring");
    let authoring_refs = authoring_dir.join("references");
    tokio::fs::create_dir_all(&authoring_refs)
        .await
        .map_err(|e| {
            FatalError::Workspace(format!(
                "failed to create skill directory {}: {e}",
                authoring_refs.display()
            ))
        })?;

    write_if_missing(&authoring_dir.join("SKILL.md"), SKILL_AUTHORING_SKILL_MD).await?;
    write_if_missing(
        &authoring_refs.join("authoring-standards.md"),
        SKILL_AUTHORING_REF_STANDARDS,
    )
    .await?;

    // workbench skill
    let workbench_dir = layout.skills_dir().join("workbench");
    let workbench_refs = workbench_dir.join("references");
    tokio::fs::create_dir_all(&workbench_refs)
        .await
        .map_err(|e| {
            FatalError::Workspace(format!(
                "failed to create skill directory {}: {e}",
                workbench_refs.display()
            ))
        })?;
    write_if_missing(&workbench_dir.join("SKILL.md"), WORKBENCH_SKILL_MD).await?;
    write_if_missing(&workbench_refs.join("api.md"), WORKBENCH_REF_API).await?;

    tracing::debug!(workspace = %layout.root().display(), "wrote bundled skills");

    Ok(())
}

/// Write content to a file only if it does not already exist.
async fn write_if_missing(path: &std::path::Path, content: &str) -> Result<(), FatalError> {
    if tokio::fs::try_exists(path)
        .await
        .map_err(|e| FatalError::Workspace(format!("failed to check {}: {e}", path.display())))?
    {
        tracing::trace!(path = %path.display(), "identity file already exists, skipping");
    } else {
        tokio::fs::write(path, content).await.map_err(|e| {
            FatalError::Workspace(format!("failed to write default {}: {e}", path.display()))
        })?;
        tracing::debug!(path = %path.display(), "created default identity file");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bootstrap_creates_structure() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("workspace"));

        ensure_workspace(&layout, None, None).await.unwrap();

        assert!(layout.root().exists(), "root should exist");
        assert!(layout.memory_dir().exists(), "memory dir should exist");
        assert!(layout.episodes_dir().exists(), "episodes dir should exist");
        assert!(layout.skills_dir().exists(), "skills dir should exist");
        assert!(layout.soul_md().exists(), "SOUL.md should exist");
        assert!(layout.agents_md().exists(), "AGENTS.md should exist");
        assert!(layout.user_md().exists(), "USER.md should exist");
        assert!(layout.wiki_dir().exists(), "wiki dir should exist");
        assert!(
            layout.wiki_index_md().exists(),
            "wiki/index.md should exist"
        );
        assert!(layout.bootstrap_md().exists(), "BOOTSTRAP.md should exist");
        assert!(layout.observer_md().exists(), "OBSERVER.md should exist");
        assert!(layout.reflector_md().exists(), "REFLECTOR.md should exist");
        assert!(
            layout.heartbeat_yml().exists(),
            "HEARTBEAT.yml should exist"
        );
        assert!(
            layout.subconscious_md().exists(),
            "SUBCONSCIOUS.md should exist"
        );
        assert!(layout.agent_inbox_dir().exists(), "inbox dir should exist");
        assert!(
            layout.agent_inbox_archive_dir().exists(),
            "inbox archive dir should exist"
        );

        let soul = tokio::fs::read_to_string(layout.soul_md()).await.unwrap();
        assert!(!soul.is_empty(), "SOUL.md should have default content");
        let user = tokio::fs::read_to_string(layout.user_md()).await.unwrap();
        assert!(!user.is_empty(), "USER.md should have default content");
    }

    #[tokio::test]
    async fn bootstrap_creates_role_skills() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("workspace"));

        ensure_workspace(&layout, None, None).await.unwrap();

        for name in ["introspection", "learner", "memory-analyst", "wiki"] {
            let skill_path = layout.skills_dir().join(name).join("SKILL.md");
            assert!(skill_path.exists(), "{name}/SKILL.md should be created");

            let content = tokio::fs::read_to_string(&skill_path).await.unwrap();
            assert!(
                content.contains(&format!("name: {name}")),
                "{name}/SKILL.md should carry its own frontmatter name"
            );
        }
    }

    #[tokio::test]
    async fn bootstrap_does_not_overwrite_existing_role_skill() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("workspace"));

        ensure_workspace(&layout, None, None).await.unwrap();

        let skill_path = layout.skills_dir().join("introspection").join("SKILL.md");
        tokio::fs::write(&skill_path, "user-edited skill")
            .await
            .unwrap();

        ensure_workspace(&layout, None, None).await.unwrap();

        let content = tokio::fs::read_to_string(&skill_path).await.unwrap();
        assert_eq!(
            content, "user-edited skill",
            "a second bootstrap must not clobber a user-edited skill"
        );
    }

    #[tokio::test]
    async fn bootstrap_creates_bundled_skills() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("workspace"));

        ensure_workspace(&layout, None, None).await.unwrap();

        // residuum-system skill tree
        let system_dir = layout.skills_dir().join("residuum-system");
        assert!(system_dir.join("SKILL.md").exists(), "system SKILL.md");
        for (file_name, _) in SYSTEM_REFS {
            assert!(
                system_dir.join("references").join(file_name).exists(),
                "residuum-system reference {file_name} should be written"
            );
        }

        // residuum-getting-started skill tree
        let started_dir = layout.skills_dir().join("residuum-getting-started");
        assert!(
            started_dir.join("SKILL.md").exists(),
            "getting-started SKILL.md"
        );
        assert!(
            started_dir.join("workflows/getting-organized.md").exists(),
            "getting-organized.md"
        );
        assert!(
            started_dir.join("workflows/monitoring-setup.md").exists(),
            "monitoring-setup.md"
        );
        assert!(
            started_dir
                .join("workflows/extending-capabilities.md")
                .exists(),
            "extending-capabilities.md"
        );
        assert!(
            started_dir
                .join("workflows/understanding-the-agent.md")
                .exists(),
            "understanding-the-agent.md"
        );
        assert!(
            started_dir
                .join("workflows/always-on-assistant.md")
                .exists(),
            "always-on-assistant.md"
        );

        // skill-authoring skill tree
        let authoring_dir = layout.skills_dir().join("skill-authoring");
        assert!(
            authoring_dir.join("SKILL.md").exists(),
            "skill-authoring SKILL.md"
        );
        assert!(
            authoring_dir
                .join("references/authoring-standards.md")
                .exists(),
            "authoring-standards.md"
        );

        // workbench skill tree
        let workbench_dir = layout.skills_dir().join("workbench");
        let workbench_skill = tokio::fs::read_to_string(workbench_dir.join("SKILL.md"))
            .await
            .unwrap();
        assert!(
            workbench_skill.contains("name: workbench"),
            "workbench SKILL.md carries its frontmatter name"
        );
        assert!(
            workbench_dir.join("references/api.md").exists(),
            "workbench api.md"
        );
        assert!(
            layout.workbench_dir().is_dir(),
            "the workbench folder exists for tools"
        );

        let system_skill_content = tokio::fs::read_to_string(system_dir.join("SKILL.md"))
            .await
            .unwrap();
        assert!(
            !system_skill_content.is_empty(),
            "system SKILL.md should have content"
        );
    }

    #[test]
    fn system_skill_links_match_bundled_references() {
        let linked: std::collections::BTreeSet<&str> = SYSTEM_SKILL_MD
            .split("(references/")
            .skip(1)
            .filter_map(|rest| rest.split_once(')').map(|(name, _)| name))
            .collect();
        let bundled: std::collections::BTreeSet<&str> =
            SYSTEM_REFS.iter().map(|(name, _)| *name).collect();
        assert_eq!(
            linked, bundled,
            "every reference linked from residuum-system SKILL.md must be bundled, and every bundled reference linked"
        );
    }

    #[tokio::test]
    async fn bootstrap_seeds_wiki() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("workspace"));

        ensure_workspace(&layout, None, None).await.unwrap();

        let index = tokio::fs::read_to_string(layout.wiki_index_md())
            .await
            .unwrap();
        assert!(
            index.contains("okf_version"),
            "root wiki index should declare its OKF version"
        );
        assert!(layout.wiki_log_md().exists(), "wiki/log.md should exist");
        assert!(
            layout.skills_dir().join("wiki/SKILL.md").exists(),
            "wiki skill should be bundled"
        );
    }

    #[tokio::test]
    async fn bootstrap_does_not_recreate_bootstrap_md_after_deletion() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("workspace"));

        // First run: BOOTSTRAP.md and sentinel are created
        ensure_workspace(&layout, None, None).await.unwrap();
        assert!(
            layout.bootstrap_md().exists(),
            "BOOTSTRAP.md should exist on first run"
        );
        assert!(
            layout.root().join(".bootstrapped").exists(),
            "sentinel should exist after first run"
        );

        // Simulate agent deleting BOOTSTRAP.md after first conversation
        tokio::fs::remove_file(layout.bootstrap_md()).await.unwrap();
        assert!(
            !layout.bootstrap_md().exists(),
            "BOOTSTRAP.md should be deleted"
        );

        // Second run: BOOTSTRAP.md should NOT be recreated
        ensure_workspace(&layout, None, None).await.unwrap();
        assert!(
            !layout.bootstrap_md().exists(),
            "BOOTSTRAP.md should not be recreated after sentinel exists"
        );
    }

    #[tokio::test]
    async fn bootstrap_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("workspace"));

        ensure_workspace(&layout, None, None).await.unwrap();

        // Modify SOUL.md
        tokio::fs::write(layout.soul_md(), "custom soul content")
            .await
            .unwrap();

        // Modify a skill file
        let system_skill = layout.skills_dir().join("residuum-system").join("SKILL.md");
        tokio::fs::write(&system_skill, "user-edited skill")
            .await
            .unwrap();

        // Run again
        ensure_workspace(&layout, None, None).await.unwrap();

        // Custom content should be preserved
        let content = tokio::fs::read_to_string(layout.soul_md()).await.unwrap();
        assert_eq!(
            content, "custom soul content",
            "existing files should not be overwritten"
        );

        let skill_content = tokio::fs::read_to_string(&system_skill).await.unwrap();
        assert_eq!(
            skill_content, "user-edited skill",
            "existing skill files should not be overwritten"
        );
    }

    #[tokio::test]
    async fn bootstrap_personalises_user_md_with_name() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("workspace"));

        ensure_workspace(&layout, Some("Alex"), None).await.unwrap();

        let content = tokio::fs::read_to_string(layout.user_md()).await.unwrap();
        assert!(
            content.contains("**Name**: Alex"),
            "USER.md should contain the user's name"
        );
    }

    #[tokio::test]
    async fn bootstrap_personalises_user_md_with_timezone() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("workspace"));

        ensure_workspace(&layout, Some("Alex"), Some("America/New_York"))
            .await
            .unwrap();

        let content = tokio::fs::read_to_string(layout.user_md()).await.unwrap();
        assert!(
            content.contains("**Name**: Alex"),
            "USER.md should contain the user's name"
        );
        assert!(
            content.contains("**Timezone**: America/New_York"),
            "USER.md should contain the timezone"
        );
    }

    #[tokio::test]
    async fn bootstrap_default_user_md_without_name() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("workspace"));

        ensure_workspace(&layout, None, None).await.unwrap();

        let content = tokio::fs::read_to_string(layout.user_md()).await.unwrap();
        assert_eq!(
            content, DEFAULT_USER,
            "USER.md should use default content when no name is provided"
        );
    }

    #[tokio::test]
    async fn bootstrap_timezone_only_user_md() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("workspace"));

        ensure_workspace(&layout, None, Some("America/New_York"))
            .await
            .unwrap();

        let content = tokio::fs::read_to_string(layout.user_md()).await.unwrap();
        assert!(content.contains("**Timezone**: America/New_York"));
        assert!(!content.contains("**Name**"));
    }

    #[tokio::test]
    async fn bootstrap_empty_string_inputs_treated_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("workspace"));

        ensure_workspace(&layout, Some(""), Some("")).await.unwrap();

        let content = tokio::fs::read_to_string(layout.user_md()).await.unwrap();
        assert_eq!(
            content, DEFAULT_USER,
            "empty strings should produce default USER.md"
        );
    }
}
