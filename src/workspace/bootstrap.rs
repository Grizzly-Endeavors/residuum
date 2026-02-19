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

Persistent notes across sessions. The agent can update this file.
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
        assert!(
            layout.para_projects_dir().exists(),
            "para/projects should exist"
        );
        assert!(layout.cron_dir().exists(), "cron dir should exist");
        assert!(layout.hooks_dir().exists(), "hooks dir should exist");
        assert!(layout.soul_md().exists(), "SOUL.md should exist");
        assert!(layout.agents_md().exists(), "AGENTS.md should exist");
        assert!(layout.user_md().exists(), "USER.md should exist");
        assert!(layout.memory_md().exists(), "MEMORY.md should exist");
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
