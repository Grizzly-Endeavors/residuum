//! Identity file loading for agent context assembly.

use crate::util::FatalError;

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
    /// MEMORY.md -- persistent memory across restarts.
    pub memory: Option<String>,
    /// ENVIRONMENT.md -- local environment notes.
    pub environment: Option<String>,
    /// BOOTSTRAP.md -- first-run guidance (present only on first conversation).
    pub bootstrap: Option<String>,
}

impl IdentityFiles {
    /// Load all identity files from the workspace.
    ///
    /// Missing files are silently treated as `None` — required files log a
    /// warning when absent.
    ///
    /// # Errors
    /// Returns `FatalError::Workspace` if a file exists but cannot be read.
    pub async fn load(layout: &WorkspaceLayout) -> Result<Self, FatalError> {
        let soul_result = read_optional(&layout.soul_md()).await?;
        let agents_result = read_optional(&layout.agents_md()).await?;
        let user_result = read_optional(&layout.user_md()).await?;
        let memory_result = read_optional(&layout.memory_md()).await?;

        if matches!(soul_result, ReadResult::Absent) {
            tracing::warn!(path = %layout.soul_md().display(), "SOUL.md is missing or empty; expected after bootstrap");
        }
        if matches!(agents_result, ReadResult::Absent) {
            tracing::warn!(path = %layout.agents_md().display(), "AGENTS.md is missing or empty; expected after bootstrap");
        }
        if matches!(user_result, ReadResult::Absent) {
            tracing::warn!(path = %layout.user_md().display(), "USER.md is missing or empty; expected after bootstrap");
        }
        if matches!(memory_result, ReadResult::Absent) {
            tracing::warn!(path = %layout.memory_md().display(), "MEMORY.md is missing or empty; expected after bootstrap");
        }

        let environment_result = read_optional(&layout.environment_md()).await?;
        if matches!(environment_result, ReadResult::Absent) {
            tracing::warn!(path = %layout.environment_md().display(), "ENVIRONMENT.md is missing or empty; expected after bootstrap");
        }

        let bootstrap = read_optional(&layout.bootstrap_md()).await?.into_option();

        Ok(Self {
            soul: soul_result.into_option(),
            agents: agents_result.into_option(),
            user: user_result.into_option(),
            memory: memory_result.into_option(),
            environment: environment_result.into_option(),
            bootstrap,
        })
    }
}

enum ReadResult {
    Present(String),
    Absent,
    WhitespaceOnly,
}

impl ReadResult {
    fn into_option(self) -> Option<String> {
        match self {
            Self::Present(s) => Some(s),
            Self::Absent | Self::WhitespaceOnly => None,
        }
    }
}

/// Read a file if it exists, returning `None` if missing.
async fn read_optional(path: &std::path::Path) -> Result<ReadResult, FatalError> {
    match tokio::fs::read_to_string(path).await {
        Ok(content) => {
            if content.trim().is_empty() {
                tracing::warn!(path = %path.display(), "identity file exists but is whitespace-only, treating as absent");
                Ok(ReadResult::WhitespaceOnly)
            } else {
                Ok(ReadResult::Present(content))
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ReadResult::Absent),
        Err(e) => Err(FatalError::Workspace(format!(
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

        ensure_workspace(&layout, None, None).await.unwrap();
        let identity = IdentityFiles::load(&layout).await.unwrap();

        assert!(identity.soul.is_some(), "soul should be loaded");
        assert!(identity.agents.is_some(), "agents should be loaded");
        assert!(identity.user.is_some(), "user should be loaded");
        assert!(identity.memory.is_some(), "memory should be loaded");
        assert!(
            identity.environment.is_some(),
            "environment should be loaded after bootstrap"
        );
        assert!(
            identity.bootstrap.is_some(),
            "bootstrap should be loaded on first run"
        );
    }

    #[tokio::test]
    async fn load_bootstrap_none_after_deletion() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("workspace"));

        ensure_workspace(&layout, None, None).await.unwrap();
        tokio::fs::remove_file(layout.bootstrap_md()).await.unwrap();

        let identity = IdentityFiles::load(&layout).await.unwrap();
        assert!(
            identity.bootstrap.is_none(),
            "bootstrap should be None after file deletion"
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
