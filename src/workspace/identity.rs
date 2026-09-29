//! Identity file loading for agent context assembly.

use crate::util::FatalError;

use super::layout::WorkspaceLayout;

/// Loaded identity files: the agent's own `SOUL.md` and `BOOTSTRAP.md`, plus
/// the team layer's `AGENTS.md`, `USER.md` and wiki index.
///
/// Each field holds the file content if the file exists, or `None` if absent.
#[derive(Debug, Clone, Default)]
pub struct IdentityFiles {
    /// SOUL.md -- core agent identity and personality.
    pub soul: Option<String>,
    /// `team/AGENTS.md` -- team-wide rules shared by every agent.
    pub agents: Option<String>,
    /// `team/USER.md` -- the user's core facts, shared by every agent.
    pub user: Option<String>,
    /// `team/wiki/index.md` -- the team wiki's root catalog. Pages are read on demand.
    pub wiki_index: Option<String>,
    /// BOOTSTRAP.md -- first-run guidance (present only on first conversation).
    pub bootstrap: Option<String>,
}

impl IdentityFiles {
    /// Load the identity files: SOUL.md and BOOTSTRAP.md from the agent's
    /// directory, AGENTS.md, USER.md and the wiki index from the team layer.
    ///
    /// Missing files are silently treated as `None`. This runs on every turn
    /// (main agent and sub-agent spawn), so it stays quiet — call
    /// [`Self::warn_missing`] once at startup to surface absent required files.
    ///
    /// # Errors
    /// Returns `FatalError::Workspace` if a file exists but cannot be read.
    #[tracing::instrument(skip_all, fields(workspace = %layout.root().display()))]
    pub async fn load(layout: &WorkspaceLayout) -> Result<Self, FatalError> {
        let soul_result = read_optional(&layout.soul_md()).await?;
        let team = layout.team();
        let agents_result = read_optional(&team.agents_md()).await?;
        let user_result = read_optional(&team.user_md()).await?;
        let wiki_index_result = read_optional(&team.wiki_index_md()).await?;

        let bootstrap = read_optional(&layout.bootstrap_md()).await?.into_option();

        Ok(Self {
            soul: soul_result.into_option(),
            agents: agents_result.into_option(),
            user: user_result.into_option(),
            wiki_index: wiki_index_result.into_option(),
            bootstrap,
        })
    }

    /// Log a warning for each absent required identity file.
    ///
    /// Called once at startup (not per turn) so the diagnostic is visible
    /// without spamming the logs on every turn's reload. BOOTSTRAP.md is
    /// excluded — it is expected to be absent after the first conversation.
    pub fn warn_missing(&self, layout: &WorkspaceLayout) {
        if self.soul.is_none() {
            tracing::warn!(path = %layout.soul_md().display(), "SOUL.md is missing or empty; expected after bootstrap");
        }
        let team = layout.team();
        if self.agents.is_none() {
            tracing::warn!(path = %team.agents_md().display(), "AGENTS.md is missing or empty; expected after bootstrap");
        }
        if self.user.is_none() {
            tracing::warn!(path = %team.user_md().display(), "USER.md is missing or empty; expected after bootstrap");
        }
        if self.wiki_index.is_none() {
            tracing::warn!(path = %team.wiki_index_md().display(), "team/wiki/index.md is missing or empty; expected after bootstrap");
        }
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
                tracing::debug!(path = %path.display(), "identity file exists but is whitespace-only, treating as absent");
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
mod tests {
    use super::*;
    use crate::workspace::bootstrap::ensure_workspace;

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
    async fn load_reads_wiki_root_index() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("workspace"));

        ensure_workspace(&layout, None, None).await.unwrap();
        tokio::fs::create_dir_all(layout.team().wiki_dir())
            .await
            .unwrap();
        tokio::fs::write(
            layout.team().wiki_index_md(),
            "# Wiki\n\n- [Homelab](/homelab/index.md): the user's k8s cluster\n",
        )
        .await
        .unwrap();

        let identity = IdentityFiles::load(&layout).await.unwrap();
        assert!(
            identity
                .wiki_index
                .as_deref()
                .is_some_and(|idx| idx.contains("/homelab/index.md")),
            "wiki_index should hold the root index.md content"
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

    #[tokio::test]
    async fn load_reads_team_files_and_agent_soul() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("scout"));
        let team = layout.team();
        tokio::fs::create_dir_all(layout.root()).await.unwrap();
        tokio::fs::create_dir_all(team.wiki_dir()).await.unwrap();
        tokio::fs::write(layout.soul_md(), "soul text")
            .await
            .unwrap();
        tokio::fs::write(layout.bootstrap_md(), "bootstrap text")
            .await
            .unwrap();
        tokio::fs::write(team.agents_md(), "team rules")
            .await
            .unwrap();
        tokio::fs::write(team.user_md(), "team user").await.unwrap();
        tokio::fs::write(team.wiki_index_md(), "team index")
            .await
            .unwrap();

        let identity = IdentityFiles::load(&layout).await.unwrap();
        assert_eq!(identity.soul.as_deref(), Some("soul text"));
        assert_eq!(identity.bootstrap.as_deref(), Some("bootstrap text"));
        assert_eq!(identity.agents.as_deref(), Some("team rules"));
        assert_eq!(identity.user.as_deref(), Some("team user"));
        assert_eq!(identity.wiki_index.as_deref(), Some("team index"));
    }

    #[tokio::test]
    async fn load_ignores_agent_dir_copies_of_team_files() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path().join("scout"));
        tokio::fs::create_dir_all(layout.root()).await.unwrap();
        tokio::fs::write(layout.root().join("AGENTS.md"), "stale")
            .await
            .unwrap();
        tokio::fs::write(layout.root().join("USER.md"), "stale")
            .await
            .unwrap();

        let identity = IdentityFiles::load(&layout).await.unwrap();
        assert!(identity.agents.is_none(), "agent-dir AGENTS.md is not read");
        assert!(identity.user.is_none(), "agent-dir USER.md is not read");
    }
}
