//! Workspace bootstrapping: creates required directories and default identity files.

use crate::util::FatalError;

use super::layout::WorkspaceLayout;

// ── Workspace bootstrap content (embedded at compile time from assets/) ──────

const DEFAULT_SOUL: &str = include_str!("../../assets/workspace-bootstrap/SOUL.md");
const DEFAULT_AGENTS: &str = include_str!("../../assets/workspace-bootstrap/AGENTS.md");
const DEFAULT_USER: &str = include_str!("../../assets/workspace-bootstrap/USER.md");
const DEFAULT_MEMORY: &str = include_str!("../../assets/workspace-bootstrap/MEMORY.md");
const DEFAULT_ENVIRONMENT: &str = include_str!("../../assets/workspace-bootstrap/ENVIRONMENT.md");

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
const DEFAULT_PRESENCE: &str = include_str!("../../assets/workspace-bootstrap/PRESENCE.toml");
const DEFAULT_ALERTS: &str = include_str!("../../assets/workspace-bootstrap/ALERTS.md");

// ── Bundled skill content (embedded at compile time from assets/) ────────────

// residuum-system skill
const SYSTEM_SKILL_MD: &str = include_str!("../../assets/bundled-skills/residuum-system/SKILL.md");
const SYSTEM_REF_MEMORY: &str =
    include_str!("../../assets/bundled-skills/residuum-system/references/memory-system.md");
const SYSTEM_REF_PROJECTS: &str =
    include_str!("../../assets/bundled-skills/residuum-system/references/projects.md");
const SYSTEM_REF_HEARTBEATS: &str =
    include_str!("../../assets/bundled-skills/residuum-system/references/heartbeats.md");
const SYSTEM_REF_INBOX: &str =
    include_str!("../../assets/bundled-skills/residuum-system/references/inbox.md");
const SYSTEM_REF_ACTIONS: &str =
    include_str!("../../assets/bundled-skills/residuum-system/references/scheduled-actions.md");
const SYSTEM_REF_SKILLS: &str =
    include_str!("../../assets/bundled-skills/residuum-system/references/skills.md");
const SYSTEM_REF_NOTIFICATIONS: &str =
    include_str!("../../assets/bundled-skills/residuum-system/references/notifications.md");
const SYSTEM_REF_BACKGROUND: &str =
    include_str!("../../assets/bundled-skills/residuum-system/references/background-tasks.md");

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

    write_if_missing(&layout.memory_md(), DEFAULT_MEMORY).await?;
    write_if_missing(&layout.environment_md(), DEFAULT_ENVIRONMENT).await?;

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
    write_if_missing(&layout.alerts_md(), DEFAULT_ALERTS).await?;
    write_if_missing(&layout.presence_toml(), DEFAULT_PRESENCE).await?;

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

    let mut out = String::from("# User Preferences\n");
    if let Some(name) = name {
        out.push_str("\n**Name**: ");
        out.push_str(name);
    }
    if let Some(tz) = tz {
        out.push_str("\n**Timezone**: ");
        out.push_str(tz);
    }
    out.push_str("\n\nUpdate this file as you learn about the user — preferences, communication style, context about their work and life.\n");
    out
}

/// Write bundled skill trees to the workspace skills directory.
///
/// Each file is written with `write_if_missing`, so user edits are preserved
/// and files are only recreated if deleted.
async fn write_bundled_skills(layout: &WorkspaceLayout) -> Result<(), FatalError> {
    // residuum-system skill
    let system_dir = layout.residuum_system_skill_dir();
    let system_refs = system_dir.join("references");
    tokio::fs::create_dir_all(&system_refs).await.map_err(|e| {
        FatalError::Workspace(format!(
            "failed to create skill directory {}: {e}",
            system_refs.display()
        ))
    })?;

    write_if_missing(&system_dir.join("SKILL.md"), SYSTEM_SKILL_MD).await?;
    write_if_missing(&system_refs.join("memory-system.md"), SYSTEM_REF_MEMORY).await?;
    write_if_missing(&system_refs.join("projects.md"), SYSTEM_REF_PROJECTS).await?;
    write_if_missing(&system_refs.join("heartbeats.md"), SYSTEM_REF_HEARTBEATS).await?;
    write_if_missing(&system_refs.join("inbox.md"), SYSTEM_REF_INBOX).await?;
    write_if_missing(
        &system_refs.join("scheduled-actions.md"),
        SYSTEM_REF_ACTIONS,
    )
    .await?;
    write_if_missing(&system_refs.join("skills.md"), SYSTEM_REF_SKILLS).await?;
    write_if_missing(
        &system_refs.join("notifications.md"),
        SYSTEM_REF_NOTIFICATIONS,
    )
    .await?;
    write_if_missing(
        &system_refs.join("background-tasks.md"),
        SYSTEM_REF_BACKGROUND,
    )
    .await?;

    // residuum-getting-started skill
    let started_dir = layout.residuum_getting_started_skill_dir();
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
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
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
        assert!(layout.projects_dir().exists(), "projects dir should exist");
        assert!(layout.archive_dir().exists(), "archive dir should exist");
        assert!(layout.soul_md().exists(), "SOUL.md should exist");
        assert!(layout.agents_md().exists(), "AGENTS.md should exist");
        assert!(layout.user_md().exists(), "USER.md should exist");
        assert!(layout.memory_md().exists(), "MEMORY.md should exist");
        assert!(
            layout.environment_md().exists(),
            "ENVIRONMENT.md should exist"
        );
        assert!(layout.bootstrap_md().exists(), "BOOTSTRAP.md should exist");
        assert!(layout.observer_md().exists(), "OBSERVER.md should exist");
        assert!(layout.reflector_md().exists(), "REFLECTOR.md should exist");
        assert!(
            layout.heartbeat_yml().exists(),
            "HEARTBEAT.yml should exist"
        );
        assert!(layout.alerts_md().exists(), "ALERTS.md should exist");
        assert!(
            layout.presence_toml().exists(),
            "PRESENCE.toml should exist"
        );
        assert!(layout.agent_inbox_dir().exists(), "inbox dir should exist");
        assert!(
            layout.agent_inbox_archive_dir().exists(),
            "inbox archive dir should exist"
        );

        let soul = tokio::fs::read_to_string(layout.soul_md()).await.unwrap();
        assert!(!soul.is_empty(), "SOUL.md should have default content");
        let memory = tokio::fs::read_to_string(layout.memory_md()).await.unwrap();
        assert!(!memory.is_empty(), "MEMORY.md should have default content");
    }

    #[tokio::test]
    async fn bootstrap_creates_bundled_skills() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("workspace"));

        ensure_workspace(&layout, None, None).await.unwrap();

        // residuum-system skill tree
        let system_dir = layout.residuum_system_skill_dir();
        assert!(system_dir.join("SKILL.md").exists(), "system SKILL.md");
        assert!(
            system_dir.join("references/memory-system.md").exists(),
            "memory-system.md"
        );
        assert!(
            system_dir.join("references/projects.md").exists(),
            "projects.md"
        );
        assert!(
            system_dir.join("references/heartbeats.md").exists(),
            "heartbeats.md"
        );
        assert!(system_dir.join("references/inbox.md").exists(), "inbox.md");
        assert!(
            system_dir.join("references/scheduled-actions.md").exists(),
            "scheduled-actions.md"
        );
        assert!(
            system_dir.join("references/skills.md").exists(),
            "skills.md"
        );
        assert!(
            system_dir.join("references/notifications.md").exists(),
            "notifications.md"
        );
        assert!(
            system_dir.join("references/background-tasks.md").exists(),
            "background-tasks.md"
        );

        // residuum-getting-started skill tree
        let started_dir = layout.residuum_getting_started_skill_dir();
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

        let system_skill_content = tokio::fs::read_to_string(system_dir.join("SKILL.md"))
            .await
            .unwrap();
        assert!(
            !system_skill_content.is_empty(),
            "system SKILL.md should have content"
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
        let system_skill = layout.residuum_system_skill_dir().join("SKILL.md");
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
