//! Workspace directory layout and path helpers.

use std::path::{Path, PathBuf};

/// Workspace directory layout with path helpers for identity files and storage.
#[derive(Debug, Clone)]
pub struct WorkspaceLayout {
    root: PathBuf,
}

impl WorkspaceLayout {
    /// Create a new workspace layout rooted at the given directory.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Root directory of the workspace.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Path to SOUL.md -- core agent identity and personality.
    #[must_use]
    pub fn soul_md(&self) -> PathBuf {
        self.root.join("SOUL.md")
    }

    /// Path to AGENTS.md -- agent capabilities and behavior rules.
    #[must_use]
    pub fn agents_md(&self) -> PathBuf {
        self.root.join("AGENTS.md")
    }

    /// Path to USER.md -- user preferences and context.
    #[must_use]
    pub fn user_md(&self) -> PathBuf {
        self.root.join("USER.md")
    }

    /// Path to MEMORY.md -- persistent memory across sessions.
    #[must_use]
    pub fn memory_md(&self) -> PathBuf {
        self.root.join("MEMORY.md")
    }

    /// Path to TOOLS.md -- tool usage guidelines.
    #[must_use]
    pub fn tools_md(&self) -> PathBuf {
        self.root.join("TOOLS.md")
    }

    /// Path to the memory directory for episodes and persistent state.
    #[must_use]
    pub fn memory_dir(&self) -> PathBuf {
        self.root.join("memory")
    }

    /// Path to the memory episodes directory.
    #[must_use]
    pub fn episodes_dir(&self) -> PathBuf {
        self.root.join("memory/episodes")
    }

    /// Path to the skills directory.
    #[must_use]
    pub fn skills_dir(&self) -> PathBuf {
        self.root.join("skills")
    }

    /// Path to the PARA projects directory.
    #[must_use]
    pub fn para_projects_dir(&self) -> PathBuf {
        self.root.join("para/projects")
    }

    /// Path to the PARA areas directory.
    #[must_use]
    pub fn para_areas_dir(&self) -> PathBuf {
        self.root.join("para/areas")
    }

    /// Path to the PARA resources directory.
    #[must_use]
    pub fn para_resources_dir(&self) -> PathBuf {
        self.root.join("para/resources")
    }

    /// Path to the PARA archive directory.
    #[must_use]
    pub fn para_archive_dir(&self) -> PathBuf {
        self.root.join("para/archive")
    }

    /// Path to the cron directory for scheduled tasks.
    #[must_use]
    pub fn cron_dir(&self) -> PathBuf {
        self.root.join("cron")
    }

    /// Path to the hooks directory.
    #[must_use]
    pub fn hooks_dir(&self) -> PathBuf {
        self.root.join("hooks")
    }

    /// All directories that should exist in a bootstrapped workspace.
    #[must_use]
    pub fn required_dirs(&self) -> Vec<PathBuf> {
        vec![
            self.root.clone(),
            self.memory_dir(),
            self.episodes_dir(),
            self.skills_dir(),
            self.para_projects_dir(),
            self.para_areas_dir(),
            self.para_resources_dir(),
            self.para_archive_dir(),
            self.cron_dir(),
            self.hooks_dir(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_paths() {
        let layout = WorkspaceLayout::new("/tmp/ws");
        assert_eq!(
            layout.soul_md(),
            PathBuf::from("/tmp/ws/SOUL.md"),
            "soul_md path"
        );
        assert_eq!(
            layout.agents_md(),
            PathBuf::from("/tmp/ws/AGENTS.md"),
            "agents_md path"
        );
        assert_eq!(
            layout.user_md(),
            PathBuf::from("/tmp/ws/USER.md"),
            "user_md path"
        );
        assert_eq!(
            layout.memory_dir(),
            PathBuf::from("/tmp/ws/memory"),
            "memory_dir path"
        );
    }

    #[test]
    fn required_dirs_count() {
        let layout = WorkspaceLayout::new("/tmp/ws");
        let dirs = layout.required_dirs();
        assert_eq!(dirs.len(), 10, "should have all required directories");
        assert!(
            dirs.contains(&PathBuf::from("/tmp/ws")),
            "root should be included"
        );
    }
}
