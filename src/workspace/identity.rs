//! Identity file loading for agent context assembly.

use crate::error::IronclawError;

use super::layout::WorkspaceLayout;

/// Loaded identity files from the workspace.
///
/// Each field holds the file content if the file exists, or `None` if absent.
#[derive(Debug, Clone, Default)]
pub struct IdentityFiles {
    /// SOUL.md -- core agent identity and personality.
    pub soul: Option<String>,
    /// AGENTS.md -- agent capabilities and behavior rules.
    pub agents: Option<String>,
    /// USER.md -- user preferences and context.
    pub user: Option<String>,
    /// MEMORY.md -- persistent memory across sessions.
    pub memory: Option<String>,
    /// TOOLS.md -- tool usage guidelines (optional).
    pub tools: Option<String>,
}

impl IdentityFiles {
    /// Load all identity files from the workspace.
    ///
    /// Missing files are silently treated as `None` (only the required ones
    /// are created during bootstrap; TOOLS.md is always optional).
    ///
    /// # Errors
    /// Returns `IronclawError::Workspace` if a file exists but cannot be read.
    pub async fn load(layout: &WorkspaceLayout) -> Result<Self, IronclawError> {
        Ok(Self {
            soul: read_optional(&layout.soul_md()).await?,
            agents: read_optional(&layout.agents_md()).await?,
            user: read_optional(&layout.user_md()).await?,
            memory: read_optional(&layout.memory_md()).await?,
            tools: read_optional(&layout.tools_md()).await?,
        })
    }
}

/// Read a file if it exists, returning `None` if missing.
async fn read_optional(path: &std::path::Path) -> Result<Option<String>, IronclawError> {
    match tokio::fs::read_to_string(path).await {
        Ok(content) => {
            if content.trim().is_empty() {
                Ok(None)
            } else {
                Ok(Some(content))
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(IronclawError::Workspace(format!(
            "failed to read {}: {e}",
            path.display()
        ))),
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use crate::workspace::bootstrap::ensure_workspace;

    #[tokio::test]
    async fn load_after_bootstrap() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("workspace"));

        ensure_workspace(&layout).await.unwrap();
        let identity = IdentityFiles::load(&layout).await.unwrap();

        assert!(identity.soul.is_some(), "soul should be loaded");
        assert!(identity.agents.is_some(), "agents should be loaded");
        assert!(identity.user.is_some(), "user should be loaded");
        assert!(identity.memory.is_some(), "memory should be loaded");
        assert!(
            identity.tools.is_none(),
            "tools should be None (not created by bootstrap)"
        );
    }

    #[tokio::test]
    async fn load_from_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());

        let identity = IdentityFiles::load(&layout).await.unwrap();

        assert!(identity.soul.is_none(), "missing soul should be None");
        assert!(identity.agents.is_none(), "missing agents should be None");
    }

    #[tokio::test]
    async fn load_skips_empty_files() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());

        tokio::fs::write(layout.soul_md(), "   \n  ").await.unwrap();

        let identity = IdentityFiles::load(&layout).await.unwrap();
        assert!(
            identity.soul.is_none(),
            "whitespace-only file should be treated as absent"
        );
    }
}
