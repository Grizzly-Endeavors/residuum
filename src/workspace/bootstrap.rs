//! Workspace bootstrapping: creates required directories and default identity files.

use crate::error::IronclawError;

use super::layout::WorkspaceLayout;

/// Default content for SOUL.md when creating a new workspace.
const DEFAULT_SOUL: &str = "\
# Soul

You are IronClaw, a personal AI assistant. You are helpful, direct, and concise.
You use tools when needed to accomplish tasks the user requests.
";

/// Default content for AGENTS.md when creating a new workspace.
const DEFAULT_AGENTS: &str = "\
# Agent Behavior

- Always explain what you're about to do before using tools
- Ask for confirmation before destructive operations
- Report errors clearly with context
";

/// Default content for USER.md when creating a new workspace.
const DEFAULT_USER: &str = "\
# User Preferences

Add your preferences here. This file is loaded into the agent's context.
";

/// Default content for MEMORY.md when creating a new workspace.
const DEFAULT_MEMORY: &str = "\
# Memory

Persistent notes across restarts. The agent can update this file.
";

/// Default content for IDENTITY.md when creating a new workspace.
const DEFAULT_IDENTITY: &str = "\
# Identity

Describe yourself here. This file can be updated as your understanding of your role evolves.
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
# The agent runs these checks in the background and alerts you to findings.
#
# Example:
#
# pulses:
#   - name: email_check
#     enabled: true
#     schedule: \"30m\"
#     active_hours: \"08:00-18:00\"
#     tasks:
#       - name: check_inbox
#         prompt: \"Check my email for urgent messages. Report anything requiring action.\"
#         alert: high
#
# schedule: duration string — \"30m\", \"2h\", \"24h\"
# active_hours: optional time window — \"HH:MM-HH:MM\" (UTC)
# alert: high | medium | low

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

/// Default content for Alerts.md when creating a new workspace.
const DEFAULT_ALERTS: &str = "\
# Alerts.md — Alert delivery behavior

This file is injected into the agent's prompt when running pulse checks.
Use it to customize how the agent reports findings.

## Guidelines

- If you find nothing noteworthy, respond with exactly: HEARTBEAT_OK
- For high-priority findings, be specific and actionable
- Keep reports concise — one paragraph per finding
- Include timestamps when relevant
";

/// Ensure the workspace directory structure exists with default identity files.
///
/// This is idempotent: existing files and directories are not modified.
///
/// # Errors
/// Returns `IronclawError::Workspace` if directories cannot be created or
/// default files cannot be written.
pub async fn ensure_workspace(layout: &WorkspaceLayout) -> Result<(), IronclawError> {
    // Create all required directories
    for dir in layout.required_dirs() {
        tokio::fs::create_dir_all(&dir).await.map_err(|e| {
            IronclawError::Workspace(format!("failed to create directory {}: {e}", dir.display()))
        })?;
    }

    // Create default identity files if they don't exist
    write_if_missing(&layout.soul_md(), DEFAULT_SOUL).await?;
    write_if_missing(&layout.agents_md(), DEFAULT_AGENTS).await?;
    write_if_missing(&layout.user_md(), DEFAULT_USER).await?;
    write_if_missing(&layout.memory_md(), DEFAULT_MEMORY).await?;
    write_if_missing(&layout.identity_md(), DEFAULT_IDENTITY).await?;
    write_if_missing(&layout.observer_md(), DEFAULT_OBSERVER_PROMPT).await?;
    write_if_missing(&layout.reflector_md(), DEFAULT_REFLECTOR_PROMPT).await?;
    write_if_missing(&layout.heartbeat_yml(), DEFAULT_HEARTBEAT).await?;
    write_if_missing(&layout.alerts_md(), DEFAULT_ALERTS).await?;
    write_if_missing(&layout.presence_toml(), DEFAULT_PRESENCE).await?;

    tracing::info!(
        workspace = %layout.root().display(),
        "workspace ready"
    );

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

        ensure_workspace(&layout).await.unwrap();

        assert!(layout.root().exists(), "root should exist");
        assert!(layout.memory_dir().exists(), "memory dir should exist");
        assert!(layout.episodes_dir().exists(), "episodes dir should exist");
        assert!(layout.skills_dir().exists(), "skills dir should exist");
        assert!(layout.projects_dir().exists(), "projects dir should exist");
        assert!(layout.archive_dir().exists(), "archive dir should exist");
        assert!(layout.cron_dir().exists(), "cron dir should exist");
        assert!(layout.hooks_dir().exists(), "hooks dir should exist");
        assert!(layout.soul_md().exists(), "SOUL.md should exist");
        assert!(layout.agents_md().exists(), "AGENTS.md should exist");
        assert!(layout.user_md().exists(), "USER.md should exist");
        assert!(layout.memory_md().exists(), "MEMORY.md should exist");
        assert!(layout.identity_md().exists(), "IDENTITY.md should exist");
        assert!(layout.observer_md().exists(), "OBSERVER.md should exist");
        assert!(layout.reflector_md().exists(), "REFLECTOR.md should exist");
        assert!(
            layout.heartbeat_yml().exists(),
            "HEARTBEAT.yml should exist"
        );
        assert!(layout.alerts_md().exists(), "Alerts.md should exist");
        assert!(
            layout.presence_toml().exists(),
            "PRESENCE.toml should exist"
        );
        assert!(layout.inbox_dir().exists(), "inbox dir should exist");
    }

    #[tokio::test]
    async fn bootstrap_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("workspace"));

        ensure_workspace(&layout).await.unwrap();

        // Modify SOUL.md
        tokio::fs::write(layout.soul_md(), "custom soul content")
            .await
            .unwrap();

        // Run again
        ensure_workspace(&layout).await.unwrap();

        // Custom content should be preserved
        let content = tokio::fs::read_to_string(layout.soul_md()).await.unwrap();
        assert_eq!(
            content, "custom soul content",
            "existing files should not be overwritten"
        );
    }
}
