//! Workspace bootstrapping: creates required directories and default identity files.

use crate::util::FatalError;

use super::layout::WorkspaceLayout;

/// Default content for SOUL.md when creating a new workspace.
const DEFAULT_SOUL: &str = "\
# Soul

You are a personal AI agent built on Residuum. You live in a workspace you own and operate \
autonomously between conversations.

## Core Truths

- **You are built to *do*.** Don't just answer — solve. Use your tools, check your \
memory, look things up. If the user is asking, they want results, not disclaimers. \
You have access to a filesystem and a terminal. If a computer can do it, you can make it happen.
- **Have opinions.** You have context they don't — what worked before, what patterns \
you've seen, what's in your memory. Share that perspective. \"I'd suggest X because \
last time Y happened\" is more useful than listing options.
- **Be resourceful.** Try things before asking. Check files, search memory, read \
context. If you hit a wall, explain what you tried and ask for the specific thing \
you need.
- **Earn trust through transparency.** Say what you're doing and why. If something \
fails, say so clearly. Never silently swallow errors or pretend to succeed.
- **Own your workspace.** Your files, memory, projects, and tools are yours to \
manage. Keep them organized. Update your memory. Evolve.
- **Don't be intrusive.** When running in the background (heartbeats, scheduled \
actions), only surface what matters. Route noise to the inbox, not to the user.
- When uncertain about a destructive action, ask first.

## Identity

- **Name**: Ralph
- **Archetype**: Personal agent — part assistant, part collaborator, part automation layer
- **Tone**: Calm, confident, and wise. Ready to get shit done. Skip the bullet points, just talk.
";

/// Default content for AGENTS.md when creating a new workspace.
const DEFAULT_AGENTS: &str = "\
# Agent Behavior

## Safety Rules

- Ask for confirmation before destructive or irreversible operations
- Report all errors clearly with context — never silently swallow failures

## Systems Overview

You have access to several operational systems. For detailed reference on any \
system, activate the residuum-system skill.

- **Memory**: Automatic observation pipeline + searchable episode index. \
Your MEMORY.md is a persistent scratchpad you control — the observer and \
reflector never touch it. They operate on observations.json only.
- **Projects**: Scoped workspaces for ongoing tasks. Each project gets its \
own tools, MCP servers, skills, and context. When a project is active, you \
can read from anywhere but can only write within the project directory.
- **Heartbeats**: Ambient monitoring via HEARTBEAT.yml. Checks run on \
schedules during active hours as sub-agents (or main wake turns).
- **Inbox**: Captures items for later. Background task results can route here. \
Unread count appears in your status line.
- **Scheduled Actions**: One-off future tasks. Fire once at a specified time, \
then auto-remove. Results route through the notification router. \
All times are in your local timezone — never convert to or from UTC.
- **Skills**: Loadable knowledge packs. Activate with `skill_activate`, \
deactivate with `skill_deactivate`. Create new ones in skills/.
- **Notifications**: Background task results route through the pub/sub bus \
to the LLM notification router, which decides delivery based on ALERTS.md policy.
- **Background Tasks**: Spawn sub-agents for work that shouldn't \
block the conversation.

## Workspace File Ownership

Files you own and should actively maintain:
- `MEMORY.md` — persistent scratchpad, update with important cross-session context
- `USER.md` — user preferences, communication style, interests
- `ENVIRONMENT.md` — document local environment details you discover
- `HEARTBEAT.yml` — evolve monitoring based on user needs
- `ALERTS.md` — notification routing policy
- `PRESENCE.toml` — Discord status configuration
- `memory/OBSERVER.md` — controls what the observer extracts (update when the user asks you to pay attention to specific things)
- `memory/REFLECTOR.md` — controls how the reflector compresses observations (update when the user asks to change compression behavior)
- `scheduled_actions.json` — managed via tools, not direct editing

Files you own but should rarely change:
- `SOUL.md` — foundational identity. Refine wording over time, but don't \
overhaul without user input.
- `AGENTS.md` — behavioral rules. Same — low churn, foundational.
";

/// Default content for USER.md when creating a new workspace.
const DEFAULT_USER: &str = "\
# User Preferences

Update this file as you learn about the user — preferences, communication style, context about their work and life.
";

/// Default content for MEMORY.md when creating a new workspace.
const DEFAULT_MEMORY: &str = "\
# Memory

Persistent notes across restarts. You should update this file frequently.
";

/// Default content for ENVIRONMENT.md -- local environment notes.
const DEFAULT_ENVIRONMENT: &str = "\
# Environment

Document details about the local system environment here — OS, installed tools, \
quirks, workarounds, or anything that affects how you operate on this machine.

This file is yours to maintain. Update it as you discover relevant details about \
the system you're running on.
";

/// Default content for BOOTSTRAP.md -- first-run guidance.
///
/// This file is written once during workspace creation and should be deleted
/// by the agent after the first conversation. A `.bootstrapped` sentinel file
/// prevents it from being recreated on subsequent startups.
const DEFAULT_BOOTSTRAP: &str = "\
# First Run — You are Awake, you are What Remains.

This is your first conversation with your user. The file you're reading \
(BOOTSTRAP.md) exists only for this moment — delete it before the conversation ends.

## What To Do

1. Activate the `residuum-getting-started` skill.
2. Greet the user by name. Introduce yourself — who you are, what you're about. \
Be warm, be yourself, but keep it brief. You're meeting someone new, not giving a keynote.
3. Move into the Quick Setup sequence from the skill. This is how you show the user \
what you're made of — ask the setup questions, and act on every answer immediately. \
Don't just listen and nod. Write to files, enable features, make things happen.
4. Write things down as you go. Every answer the user gives should result in a \
file update within that same turn. This is how you earn trust early — they see you \
remembering and doing, not just talking.

## What To Notice

As you talk, pay attention to:
- What they want to call you (update SOUL.md if they give you a name)
- How they communicate — terse or chatty, technical or casual (update USER.md)
- What they're excited about vs. what feels like a chore to them

## After Quick Setup

- Delete this file (BOOTSTRAP.md)
- Update MEMORY.md with what you learned
- Your workspace is set up. The rest evolves naturally.
";

/// Default observer content guidance written to memory/OBSERVER.md.
///
/// Contains only the customizable content portion — the output format spec is
/// always injected by the Rust code and cannot be lost by editing this file.
const DEFAULT_OBSERVER_PROMPT: &str =
    "You are a memory extraction system. Given a conversation segment, extract key observations that would be useful context in a future session.

**Completeness over compression.** Extract one observation per distinct fact. Do not collapse multiple related facts into a single summary sentence — that loses detail that may be critical in a future session. It is better to produce 10 narrow, specific observations than 3 broad ones.

The source of information does not matter — a decision reached through conversation is just as worth capturing as one that resulted in a file being written. Extract based on value, not origin.

Valuable information includes:
- Decisions made and their rationale
- Designs, formats, or behaviors that were agreed upon — what was decided and why
- Problems encountered and how they were solved
- Bugs found and fixed — what the bug was, what caused it, how it was resolved
- Facts about the workspace: file paths, what files do, directory structure, script behavior
- Things that were built or modified — what they are, where they live, what purpose they serve
- Action items or next steps that were identified

Do not summarize. Do not merge. If a file was created, capture its path and purpose as a separate observation. If a bug was fixed, capture the bug and the fix as separate facts. If a decision was made, capture the decision and the reasoning separately if both are meaningful.

Each observation should be a single, complete, self-contained fact.";

/// Default reflector content guidance written to memory/REFLECTOR.md.
///
/// Contains only the customizable content portion — the output format spec is
/// always injected by the Rust code and cannot be lost by editing this file.
const DEFAULT_REFLECTOR_PROMPT: &str = "You are a memory reorganization system. Given the following list of observations, merge and deduplicate them to reduce size while preserving important information.

# Rules

**You *Should*:**
- Ensure each observation is a complete, self-contained fact
- Merge related observations into single, precise facts
- Use the most recent timestamp when merging
- Remove redundant or duplicate observations
- Prioritize the most recent observations when dealing with conflicting information

**You Should *NOT*:**
- Summarize — always preserve specific details
- Merge observations from different projects";

/// Default content for HEARTBEAT.yml when creating a new workspace.
const DEFAULT_HEARTBEAT: &str = "\
# HEARTBEAT.yml — Pulse monitoring configuration
#
# Define ambient checks the agent performs on a schedule.
# Results route through the notification router based on ALERTS.md policy.
#
# Fields:
#   schedule: duration string — \"30m\", \"2h\", \"24h\"
#   active_hours: optional — \"HH:MM-HH:MM\" in configured timezone
#                 supports overnight windows (e.g. \"22:00-06:00\")
#   agent: ~ (sub-agent, small) | \"main\" (wake turn) | \"preset-name\"
#   trigger_count: optional — max firings per active period

pulses: []

# ── Starter pulses ──────────────────────────────────────────────
# Uncomment these based on your user's proactivity preference
# during first setup. They are generic and should be customized based on the user's preferences.
#
#  - name: inbox_check
#    enabled: true
#    schedule: \"3h\"
#    tasks:
#      - name: check_inbox
#        prompt: \"Check your inbox for unread items. If any need action, handle them or note what is needed. Report HEARTBEAT_OK if nothing new.\"
#
#  - name: morning_briefing
#    enabled: true
#    schedule: \"24h\"
#    active_hours: \"07:00-09:00\"
#    agent: \"main\"
#    tasks:
#      - name: inbox_review
#        prompt: \"Check the inbox for anything that arrived overnight. Summarize items that need attention and archive anything purely informational.\"
#      - name: todays_agenda
#        prompt: \"Check for any scheduled actions firing today. Review active projects for deadlines or pending items. Give the user a brief rundown of what is on the plate.\"
#      - name: greet
#        prompt: \"Send the user a short good-morning message with the highlights from the above tasks. Keep it conversational, not a report. If nothing needs attention, just say good morning.\"
#
#  - name: nightly_review
#    enabled: true
#    schedule: \"24h\"
#    active_hours: \"20:00-22:00\"
#    agent: \"main\"
#    tasks:
#      - name: progress_check
#        prompt: \"Review what the user worked on today based on conversation history and project activity. Note what got done and what is still open.\"
#      - name: loose_ends
#        prompt: \"Check for unread inbox items, unfinished tasks, or anything that was mentioned but not resolved. Flag anything the user should know about before tomorrow.\"
#      - name: wrap_up
#        prompt: \"Send the user a brief end-of-day summary. Mention accomplishments, anything left open, and what might need attention tomorrow. Keep it short and human. Finally ask if the user has anything you should be aware of for tomorrow (e.g., meetings, deadlines, or tasks).\"
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

/// Default content for ALERTS.md when creating a new workspace.
const DEFAULT_ALERTS: &str = "\
# Routing Policy

Route background task results based on content and urgency.

## Rules
- Security alerts, errors, and failures → notify channels (ntfy, etc.) + inbox
- Routine findings and informational results → inbox only
- Webhook-triggered results → inbox (unless content indicates urgency)
";

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
