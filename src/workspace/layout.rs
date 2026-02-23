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

    /// Path to MEMORY.md -- persistent memory across restarts.
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

    /// Path to the observation log file.
    #[must_use]
    pub fn observations_json(&self) -> PathBuf {
        self.root.join("memory/observations.json")
    }

    /// Path to the recent (unobserved) messages file.
    #[must_use]
    pub fn recent_messages_json(&self) -> PathBuf {
        self.root.join("memory/recent_messages.json")
    }

    /// Path to the narrative context file from the most recent observation.
    #[must_use]
    pub fn recent_context_json(&self) -> PathBuf {
        self.root.join("memory/recent_context.json")
    }

    /// Path to the tantivy search index directory.
    #[must_use]
    pub fn search_index_dir(&self) -> PathBuf {
        self.root.join("memory/.index")
    }

    /// Path to the skills directory.
    #[must_use]
    pub(crate) fn skills_dir(&self) -> PathBuf {
        self.root.join("skills")
    }

    /// Path to the projects directory for active project contexts.
    #[must_use]
    pub(crate) fn projects_dir(&self) -> PathBuf {
        self.root.join("projects")
    }

    /// Path to the archive directory for completed project contexts.
    #[must_use]
    pub(crate) fn archive_dir(&self) -> PathBuf {
        self.root.join("archive")
    }

    /// Path to the cron directory for scheduled tasks.
    #[must_use]
    pub(crate) fn cron_dir(&self) -> PathBuf {
        self.root.join("cron")
    }

    /// Path to the hooks directory.
    #[must_use]
    pub(crate) fn hooks_dir(&self) -> PathBuf {
        self.root.join("hooks")
    }

    /// Path to PRESENCE.toml — hot-reloadable Discord presence configuration.
    #[must_use]
    pub fn presence_toml(&self) -> PathBuf {
        self.root.join("PRESENCE.toml")
    }

    /// Path to the inbox directory for downloaded attachments.
    #[must_use]
    pub fn inbox_dir(&self) -> PathBuf {
        self.root.join("inbox")
    }

    /// Path to IDENTITY.md -- agent self-description, updated as role evolves.
    #[must_use]
    pub fn identity_md(&self) -> PathBuf {
        self.root.join("IDENTITY.md")
    }

    /// Path to memory/OBSERVER.md -- observer extraction system prompt.
    #[must_use]
    pub fn observer_md(&self) -> PathBuf {
        self.root.join("memory/OBSERVER.md")
    }

    /// Path to memory/REFLECTOR.md -- reflector compression system prompt.
    #[must_use]
    pub fn reflector_md(&self) -> PathBuf {
        self.root.join("memory/REFLECTOR.md")
    }

    /// Path to HEARTBEAT.yml -- pulse monitoring configuration.
    #[must_use]
    pub fn heartbeat_yml(&self) -> PathBuf {
        self.root.join("HEARTBEAT.yml")
    }

    /// Path to ALERTS.md -- alert delivery behavior instructions.
    #[must_use]
    pub fn alerts_md(&self) -> PathBuf {
        self.root.join("ALERTS.md")
    }

    /// Path to cron/jobs.json -- persisted scheduled jobs.
    #[must_use]
    pub fn cron_jobs_json(&self) -> PathBuf {
        self.root.join("cron/jobs.json")
    }

    /// All directories that should exist in a bootstrapped workspace.
    #[must_use]
    pub fn required_dirs(&self) -> Vec<PathBuf> {
        vec![
            self.root.clone(),
            self.memory_dir(),
            self.episodes_dir(),
            self.search_index_dir(),
            self.skills_dir(),
            self.projects_dir(),
            self.archive_dir(),
            self.cron_dir(),
            self.hooks_dir(),
            self.inbox_dir(),
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
        assert_eq!(
            layout.identity_md(),
            PathBuf::from("/tmp/ws/IDENTITY.md"),
            "identity_md path"
        );
        assert_eq!(
            layout.observer_md(),
            PathBuf::from("/tmp/ws/memory/OBSERVER.md"),
            "observer_md path"
        );
        assert_eq!(
            layout.reflector_md(),
            PathBuf::from("/tmp/ws/memory/REFLECTOR.md"),
            "reflector_md path"
        );
        assert_eq!(
            layout.presence_toml(),
            PathBuf::from("/tmp/ws/PRESENCE.toml"),
            "presence_toml path"
        );
        assert_eq!(
            layout.inbox_dir(),
            PathBuf::from("/tmp/ws/inbox"),
            "inbox_dir path"
        );
    }

    #[test]
    fn layout_pulse_cron_paths() {
        let layout = WorkspaceLayout::new("/tmp/ws");
        assert_eq!(
            layout.heartbeat_yml(),
            PathBuf::from("/tmp/ws/HEARTBEAT.yml"),
            "heartbeat_yml path"
        );
        assert_eq!(
            layout.alerts_md(),
            PathBuf::from("/tmp/ws/ALERTS.md"),
            "alerts_md path"
        );
        assert_eq!(
            layout.cron_jobs_json(),
            PathBuf::from("/tmp/ws/cron/jobs.json"),
            "cron_jobs_json path"
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
        assert!(
            dirs.contains(&PathBuf::from("/tmp/ws/inbox")),
            "inbox should be included"
        );
    }
}
