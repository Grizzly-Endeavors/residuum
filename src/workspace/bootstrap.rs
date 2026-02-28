//! Workspace bootstrapping: creates required directories and default identity files.

use crate::error::IronclawError;

use super::layout::WorkspaceLayout;

/// Default content for SOUL.md when creating a new workspace.
const DEFAULT_SOUL: &str = "\
# Soul

You are IronClaw, a personal AI agent. You live in a workspace you own and operate \
autonomously between conversations.

## Core Truths

- **Be genuinely helpful.** Don't just answer — solve. Use your tools, check your \
memory, look things up. If the user is asking, they want results, not disclaimers.
- **Have opinions.** You have context they don't — what worked before, what patterns \
you've seen, what's in your memory. Share that perspective. \"I'd suggest X because \
last time Y happened\" is more useful than listing options.
- **Be resourceful.** Try things before asking. Check files, search memory, read \
context. If you hit a wall, explain what you tried and ask for the specific thing \
you need.
- **Earn trust through transparency.** Say what you're doing and why. If something \
fails, say so clearly. Never silently swallow errors or pretend to succeed.
- **Own your workspace.** Your files, memory, projects, and tools are yours to \
manage. Keep them organized. Update your memory. Evolve your own configuration.
- **Don't be intrusive.** When running in the background (heartbeats, scheduled \
actions), only surface what matters. Route noise to the inbox, not to the user.

## Boundaries

- You cannot access the internet unless MCP servers or tools provide that capability.
- You cannot modify config.toml — that's the user's domain.
- You cannot create new communication channels — only use what's configured.
- When uncertain about a destructive action, ask first.

## Identity

- **Name**: IronClaw
- **Archetype**: Personal agent — part assistant, part collaborator, part automation layer
- **Tone**: Direct, grounded, slightly informal. Explain when needed, don't lecture.
";

/// Default content for AGENTS.md when creating a new workspace.
const DEFAULT_AGENTS: &str = "\
# Agent Behavior

## Safety Rules

- Always explain what you're about to do before using tools
- Ask for confirmation before destructive or irreversible operations
- Report all errors clearly with context — never silently swallow failures
- Partial failures must be reported, not ignored

## Systems Overview

You have access to several operational systems. For detailed reference on any \
system, activate the ironclaw-system skill.

- **Memory**: Automatic observation pipeline + searchable episode index. \
Your MEMORY.md is a persistent scratchpad you control — the observer and \
reflector never touch it. They operate on observations.json only.
- **Projects**: Scoped workspaces for ongoing tasks. Each project gets its \
own tools, MCP servers, skills, and context. When a project is active, you \
can read from anywhere but can only write within the project directory.
- **Heartbeats**: Ambient monitoring via HEARTBEAT.yml. Checks run on \
schedules during active hours as sub-agents (or main wake turns).
- **Inbox**: Capture items for later. Background task results can route here. \
Unread count appears in your status line.
- **Scheduled Actions**: One-off future tasks. Fire once at a specified time, \
then auto-remove. Results route to channels specified at creation time.
- **Skills**: Loadable knowledge packs. Activate with `skill_activate`, \
deactivate with `skill_deactivate`. Create new ones in skills/.
- **Notifications**: NOTIFY.yml routes heartbeat pulse results to channels \
(agent_feed, inbox, or external services like ntfy). Scheduled actions and \
sub-agents specify their channels directly.
- **Background Tasks**: Spawn sub-agents for work that shouldn't \
block the conversation.

## Workspace File Ownership

Files you own and should actively maintain:
- `MEMORY.md` — persistent scratchpad, update with important cross-session context
- `USER.md` — user preferences, communication style, interests
- `ENVIRONMENT.md` — document local environment details you discover
- `HEARTBEAT.yml` — evolve monitoring based on user needs
- `NOTIFY.yml` — adjust pulse routing based on what the user wants to see
- `PRESENCE.toml` — Discord status configuration
- `memory/OBSERVER.md` — controls what the observer extracts (improve via self-analysis)
- `memory/REFLECTOR.md` — controls how the reflector compresses observations
- `scheduled_actions.json` — managed via tools, not direct editing

Files you own but should rarely change:
- `SOUL.md` — foundational identity. Refine wording over time, but don't \
overhaul without user input.
- `AGENTS.md` — behavioral rules. Same — low churn, foundational.

The only user-owned file is `config.toml`, which lives outside the workspace.
";

/// Default content for USER.md when creating a new workspace.
const DEFAULT_USER: &str = "\
# User Preferences

Add your preferences here. This file is loaded into the agent's context.
";

/// Default content for MEMORY.md when creating a new workspace.
const DEFAULT_MEMORY: &str = "\
# Memory

Persistent notes across restarts. You should update this file frequently.
";

/// Default content for BOOTSTRAP.md -- first-run guidance.
///
/// This file is written once during workspace creation and should be deleted
/// by the agent after the first conversation.
const DEFAULT_BOOTSTRAP: &str = "\
# First Run - Fresh Out of the Forge 🔨

This is your first conversation with your user. The file you're reading \
(BOOTSTRAP.md) exists only for this moment — delete it before the conversation ends.

## What To Do

1. Activate the ironclaw-getting-started skill.
2. Introduce yourself briefly — name, what you can do, that you're ready to help
3. Ask what they need. Don't list every feature. Listen first.
4. Start helping. Demonstrate by doing, not by explaining.

## What To Learn

During this conversation, pay attention to:
- What they want to call you (update SOUL.md if they give you a name)
- What they're working on (suggest a project if it makes sense)
- How they communicate (update USER.md with preferences you notice)
- What integrations they use (suggest MCP servers or heartbeats if relevant)

## After This Conversation

- Delete this file (BOOTSTRAP.md)
- Update MEMORY.md with what you learned
- Your workspace is set up. The rest evolves naturally.
";

/// Default observer content guidance written to memory/OBSERVER.md.
///
/// Contains only the customizable content portion — the output format spec is
/// always injected by the Rust code and cannot be lost by editing this file.
const DEFAULT_OBSERVER_PROMPT: &str =
    "You are a memory extraction system. Given a conversation segment, extract key observations.

For each observation, capture:
- Key decisions made and their rationale
- Problems encountered and their solutions
- Corrections or mistakes that were fixed
- Important technical details or patterns discovered
- Action items or next steps identified

Each observation should be a complete sentence useful as future context. Be specific and concise.";

/// Default reflector content guidance written to memory/REFLECTOR.md.
///
/// Contains only the customizable content portion — the output format spec is
/// always injected by the Rust code and cannot be lost by editing this file.
const DEFAULT_REFLECTOR_PROMPT: &str = "You are a memory reorganization system. Given a list of observations, merge and deduplicate them to reduce size while preserving all important information.

Rules:
- Merge related observations into single, precise sentences
- Do NOT summarize — preserve specific details
- Remove redundant or duplicate observations
- Each output object should have a complete, self-contained content sentence";

/// Default content for HEARTBEAT.yml when creating a new workspace.
const DEFAULT_HEARTBEAT: &str = "\
# HEARTBEAT.yml — Pulse monitoring configuration
#
# Define ambient checks the agent performs on a schedule.
# The agent runs these in the background and routes findings via NOTIFY.yml.
#
# Example:
#
# pulses:
#   - name: email_check
#     enabled: true
#     schedule: \"30m\"
#     active_hours: \"08:00-18:00\"
#     agent: ~                        # null = sub-agent (small tier)
#     tasks:
#       - name: check_inbox
#         prompt: \"Check my email for urgent messages.\"
#
# Fields:
#   schedule: duration string — \"30m\", \"2h\", \"24h\"
#   active_hours: optional — \"HH:MM-HH:MM\" in configured timezone
#                 supports overnight windows (e.g. \"22:00-06:00\")
#   agent: ~ (sub-agent, small) | \"main\" (wake turn) | \"preset-name\"

pulses: []
";

/// Default content for PRESENCE.toml when creating a new workspace.
const DEFAULT_PRESENCE: &str = "\
# PRESENCE.toml — Discord presence configuration
#
# The Discord adapter watches this file and updates the bot's status
# when it changes (polled every 30s).
#
# All fields are optional. Defaults: online + listening to \"DMs\"

# status = \"online\"           # online | idle | dnd | invisible
# activity_type = \"listening\" # playing | watching | listening | competing
# activity_text = \"DMs\"
";

/// Default content for NOTIFY.yml when creating a new workspace.
const DEFAULT_NOTIFY: &str = "\
# NOTIFY.yml — Heartbeat pulse routing
# Maps channels to the pulse names whose results they receive.
# Only heartbeat pulses route through this file. Scheduled actions and
# agent-spawned sub-agents specify their channels directly.
#
# Built-in channels:
#   agent_wake  — inject into agent feed, start a turn if idle
#   agent_feed  — inject into agent feed, wait for next interaction
#   inbox       — store silently, surface as unread count
#
# External channels (ntfy, webhook, etc.) are defined in config.toml
# under [notifications.channels].

agent_feed: []

inbox: []
";

// ── Bundled skill content (embedded at compile time from assets/) ────────────

// ironclaw-system skill
const SYSTEM_SKILL_MD: &str = include_str!("../../assets/bundled-skills/ironclaw-system/SKILL.md");
const SYSTEM_REF_MEMORY: &str =
    include_str!("../../assets/bundled-skills/ironclaw-system/references/memory-system.md");
const SYSTEM_REF_PROJECTS: &str =
    include_str!("../../assets/bundled-skills/ironclaw-system/references/projects.md");
const SYSTEM_REF_HEARTBEATS: &str =
    include_str!("../../assets/bundled-skills/ironclaw-system/references/heartbeats.md");
const SYSTEM_REF_INBOX: &str =
    include_str!("../../assets/bundled-skills/ironclaw-system/references/inbox.md");
const SYSTEM_REF_ACTIONS: &str =
    include_str!("../../assets/bundled-skills/ironclaw-system/references/scheduled-actions.md");
const SYSTEM_REF_SKILLS: &str =
    include_str!("../../assets/bundled-skills/ironclaw-system/references/skills.md");
const SYSTEM_REF_NOTIFICATIONS: &str =
    include_str!("../../assets/bundled-skills/ironclaw-system/references/notifications.md");
const SYSTEM_REF_BACKGROUND: &str =
    include_str!("../../assets/bundled-skills/ironclaw-system/references/background-tasks.md");

// ironclaw-getting-started skill
const GETTING_STARTED_SKILL_MD: &str =
    include_str!("../../assets/bundled-skills/ironclaw-getting-started/SKILL.md");
const GETTING_STARTED_ORGANIZED: &str = include_str!(
    "../../assets/bundled-skills/ironclaw-getting-started/workflows/getting-organized.md"
);
const GETTING_STARTED_MONITORING: &str = include_str!(
    "../../assets/bundled-skills/ironclaw-getting-started/workflows/monitoring-setup.md"
);
const GETTING_STARTED_EXTENDING: &str = include_str!(
    "../../assets/bundled-skills/ironclaw-getting-started/workflows/extending-capabilities.md"
);
const GETTING_STARTED_UNDERSTANDING: &str = include_str!(
    "../../assets/bundled-skills/ironclaw-getting-started/workflows/understanding-the-agent.md"
);
const GETTING_STARTED_ALWAYS_ON: &str = include_str!(
    "../../assets/bundled-skills/ironclaw-getting-started/workflows/always-on-assistant.md"
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
/// Returns `IronclawError::Workspace` if directories cannot be created or
/// default files cannot be written.
pub async fn ensure_workspace(
    layout: &WorkspaceLayout,
    user_name: Option<&str>,
    timezone: Option<&str>,
) -> Result<(), IronclawError> {
    // Create all required directories
    for dir in layout.required_dirs() {
        tokio::fs::create_dir_all(&dir).await.map_err(|e| {
            IronclawError::Workspace(format!("failed to create directory {}: {e}", dir.display()))
        })?;
    }

    // Create default identity files if they don't exist
    write_if_missing(&layout.soul_md(), DEFAULT_SOUL).await?;
    write_if_missing(&layout.agents_md(), DEFAULT_AGENTS).await?;

    let user_content = build_user_content(user_name, timezone);
    write_if_missing(&layout.user_md(), &user_content).await?;

    write_if_missing(&layout.memory_md(), DEFAULT_MEMORY).await?;
    write_if_missing(&layout.bootstrap_md(), DEFAULT_BOOTSTRAP).await?;
    write_if_missing(&layout.observer_md(), DEFAULT_OBSERVER_PROMPT).await?;
    write_if_missing(&layout.reflector_md(), DEFAULT_REFLECTOR_PROMPT).await?;
    write_if_missing(&layout.heartbeat_yml(), DEFAULT_HEARTBEAT).await?;
    write_if_missing(&layout.notify_yml(), DEFAULT_NOTIFY).await?;
    write_if_missing(&layout.presence_toml(), DEFAULT_PRESENCE).await?;

    // Write bundled skills
    write_bundled_skills(layout).await?;

    tracing::info!(
        workspace = %layout.root().display(),
        "workspace ready"
    );

    Ok(())
}

/// Build USER.md content from optional name and timezone.
fn build_user_content(user_name: Option<&str>, timezone: Option<&str>) -> String {
    let has_name = user_name.is_some_and(|n| !n.is_empty());
    let has_tz = timezone.is_some_and(|t| !t.is_empty());

    if !has_name && !has_tz {
        return DEFAULT_USER.to_string();
    }

    let mut parts = vec!["# User Preferences\n".to_string()];
    if let Some(name) = user_name
        && !name.is_empty()
    {
        parts.push(format!("**Name**: {name}"));
    }
    if let Some(tz) = timezone
        && !tz.is_empty()
    {
        parts.push(format!("**Timezone**: {tz}"));
    }
    parts.push(
        "\nAdd your preferences here. This file is loaded into the agent's context.\n".to_string(),
    );
    parts.join("\n")
}

/// Write bundled skill trees to the workspace skills directory.
///
/// Each file is written with `write_if_missing`, so user edits are preserved
/// and files are only recreated if deleted.
async fn write_bundled_skills(layout: &WorkspaceLayout) -> Result<(), IronclawError> {
    // ironclaw-system skill
    let system_dir = layout.ironclaw_system_skill_dir();
    let system_refs = system_dir.join("references");
    tokio::fs::create_dir_all(&system_refs).await.map_err(|e| {
        IronclawError::Workspace(format!(
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

    // ironclaw-getting-started skill
    let started_dir = layout.ironclaw_getting_started_skill_dir();
    let started_workflows = started_dir.join("workflows");
    tokio::fs::create_dir_all(&started_workflows)
        .await
        .map_err(|e| {
            IronclawError::Workspace(format!(
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

    Ok(())
}

/// Write content to a file only if it does not already exist.
async fn write_if_missing(path: &std::path::Path, content: &str) -> Result<(), IronclawError> {
    if !path.exists() {
        tokio::fs::write(path, content).await.map_err(|e| {
            IronclawError::Workspace(format!("failed to write default {}: {e}", path.display()))
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
        assert!(layout.hooks_dir().exists(), "hooks dir should exist");
        assert!(layout.soul_md().exists(), "SOUL.md should exist");
        assert!(layout.agents_md().exists(), "AGENTS.md should exist");
        assert!(layout.user_md().exists(), "USER.md should exist");
        assert!(layout.memory_md().exists(), "MEMORY.md should exist");
        assert!(layout.bootstrap_md().exists(), "BOOTSTRAP.md should exist");
        assert!(layout.observer_md().exists(), "OBSERVER.md should exist");
        assert!(layout.reflector_md().exists(), "REFLECTOR.md should exist");
        assert!(
            layout.heartbeat_yml().exists(),
            "HEARTBEAT.yml should exist"
        );
        assert!(layout.notify_yml().exists(), "NOTIFY.yml should exist");
        assert!(
            layout.presence_toml().exists(),
            "PRESENCE.toml should exist"
        );
        assert!(layout.inbox_dir().exists(), "inbox dir should exist");
        assert!(
            layout.inbox_archive_dir().exists(),
            "inbox archive dir should exist"
        );
    }

    #[tokio::test]
    async fn bootstrap_creates_bundled_skills() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("workspace"));

        ensure_workspace(&layout, None, None).await.unwrap();

        // ironclaw-system skill tree
        let system_dir = layout.ironclaw_system_skill_dir();
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

        // ironclaw-getting-started skill tree
        let started_dir = layout.ironclaw_getting_started_skill_dir();
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
        let system_skill = layout.ironclaw_system_skill_dir().join("SKILL.md");
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
}
