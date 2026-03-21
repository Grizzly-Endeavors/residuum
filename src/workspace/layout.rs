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

    /// Path to ENVIRONMENT.md -- local environment notes.
    #[must_use]
    pub fn environment_md(&self) -> PathBuf {
        self.root.join("ENVIRONMENT.md")
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

    /// Path to the index manifest file tracking which files have been indexed.
    #[must_use]
    pub fn index_manifest_json(&self) -> PathBuf {
        self.root.join("memory/.index_manifest.json")
    }

    /// Path to the sqlite-vec vector database file.
    #[must_use]
    pub fn vectors_db(&self) -> PathBuf {
        self.root.join("memory/vectors.db")
    }

    /// Path to the skills directory.
    #[must_use]
    pub fn skills_dir(&self) -> PathBuf {
        self.root.join("skills")
    }

    /// Path to BOOTSTRAP.md -- first-run guidance, deleted after first conversation.
    #[must_use]
    pub fn bootstrap_md(&self) -> PathBuf {
        self.root.join("BOOTSTRAP.md")
    }

    /// Path to the subagent presets directory.
    #[must_use]
    pub fn subagents_dir(&self) -> PathBuf {
        self.root.join("subagents")
    }

    /// Path to the bundled `residuum-system` skill directory.
    #[must_use]
    pub fn residuum_system_skill_dir(&self) -> PathBuf {
        self.root.join("skills/residuum-system")
    }

    /// Path to the bundled `residuum-getting-started` skill directory.
    #[must_use]
    pub fn residuum_getting_started_skill_dir(&self) -> PathBuf {
        self.root.join("skills/residuum-getting-started")
    }

    /// Path to the projects directory for active project contexts.
    #[must_use]
    pub fn projects_dir(&self) -> PathBuf {
        self.root.join("projects")
    }

    /// Path to the archive directory for completed project contexts.
    #[must_use]
    pub fn archive_dir(&self) -> PathBuf {
        self.root.join("archive")
    }

    /// Path to PRESENCE.toml — hot-reloadable Discord presence configuration.
    #[must_use]
    pub fn presence_toml(&self) -> PathBuf {
        self.root.join("PRESENCE.toml")
    }

    /// Path to the inbox directory for downloaded attachments and inbox items.
    #[must_use]
    pub fn inbox_dir(&self) -> PathBuf {
        self.root.join("inbox")
    }

    /// Path to the inbox archive directory for archived inbox items.
    #[must_use]
    pub fn inbox_archive_dir(&self) -> PathBuf {
        self.root.join("archive/inbox")
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

    /// Path to CHANNELS.yml -- channel registry.
    #[must_use]
    pub fn channels_yml(&self) -> PathBuf {
        self.root.join("CHANNELS.yml")
    }

    /// Path to ALERTS.md -- notification routing policy for the LLM router.
    #[must_use]
    pub fn alerts_md(&self) -> PathBuf {
        self.root.join("ALERTS.md")
    }

    /// Path to the workspace config directory (`root/config/`).
    #[must_use]
    pub fn config_dir(&self) -> PathBuf {
        self.root.join("config")
    }

    /// Path to `config/mcp.json` — MCP server definitions.
    #[must_use]
    pub fn mcp_json(&self) -> PathBuf {
        self.root.join("config/mcp.json")
    }

    /// Path to `config/channels.toml` — external notification channel definitions.
    #[must_use]
    pub fn channels_toml(&self) -> PathBuf {
        self.root.join("config/channels.toml")
    }

    /// Path to the background task transcript directory.
    ///
    /// Created on-demand when the first transcript is written, not at bootstrap.
    #[must_use]
    pub fn background_dir(&self) -> PathBuf {
        self.root.join("memory/background")
    }

    /// Path to `pulse_state.json` -- persisted pulse scheduler state (`last_run`, `run_counts`).
    #[must_use]
    pub fn pulse_state_json(&self) -> PathBuf {
        self.root.join("pulse_state.json")
    }

    /// Path to `scheduled_actions.json` -- persisted one-off scheduled actions.
    #[must_use]
    pub fn scheduled_actions_json(&self) -> PathBuf {
        self.root.join("scheduled_actions.json")
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
            self.subagents_dir(),
            self.projects_dir(),
            self.archive_dir(),
            self.inbox_dir(),
            self.inbox_archive_dir(),
            self.config_dir(),
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
            layout.environment_md(),
            PathBuf::from("/tmp/ws/ENVIRONMENT.md"),
            "environment_md path"
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
        assert_eq!(
            layout.inbox_archive_dir(),
            PathBuf::from("/tmp/ws/archive/inbox"),
            "inbox_archive_dir path"
        );
        assert_eq!(
            layout.vectors_db(),
            PathBuf::from("/tmp/ws/memory/vectors.db"),
            "vectors_db path"
        );
        assert_eq!(
            layout.subagents_dir(),
            PathBuf::from("/tmp/ws/subagents"),
            "subagents_dir path"
        );
        assert_eq!(
            layout.bootstrap_md(),
            PathBuf::from("/tmp/ws/BOOTSTRAP.md"),
            "bootstrap_md path"
        );
        assert_eq!(
            layout.residuum_system_skill_dir(),
            PathBuf::from("/tmp/ws/skills/residuum-system"),
            "residuum_system_skill_dir path"
        );
        assert_eq!(
            layout.residuum_getting_started_skill_dir(),
            PathBuf::from("/tmp/ws/skills/residuum-getting-started"),
            "residuum_getting_started_skill_dir path"
        );
        assert_eq!(
            layout.config_dir(),
            PathBuf::from("/tmp/ws/config"),
            "config_dir path"
        );
        assert_eq!(
            layout.mcp_json(),
            PathBuf::from("/tmp/ws/config/mcp.json"),
            "mcp_json path"
        );
        assert_eq!(
            layout.channels_toml(),
            PathBuf::from("/tmp/ws/config/channels.toml"),
            "channels_toml path"
        );
        assert_eq!(
            layout.alerts_md(),
            PathBuf::from("/tmp/ws/ALERTS.md"),
            "alerts_md path"
        );
    }

    #[test]
    fn layout_pulse_action_paths() {
        let layout = WorkspaceLayout::new("/tmp/ws");
        assert_eq!(
            layout.heartbeat_yml(),
            PathBuf::from("/tmp/ws/HEARTBEAT.yml"),
            "heartbeat_yml path"
        );
        assert_eq!(
            layout.channels_yml(),
            PathBuf::from("/tmp/ws/CHANNELS.yml"),
            "channels_yml path"
        );
        assert_eq!(
            layout.pulse_state_json(),
            PathBuf::from("/tmp/ws/pulse_state.json"),
            "pulse_state_json path"
        );
        assert_eq!(
            layout.scheduled_actions_json(),
            PathBuf::from("/tmp/ws/scheduled_actions.json"),
            "scheduled_actions_json path"
        );
    }

    #[test]
    fn required_dirs_all_under_root() {
        let layout = WorkspaceLayout::new("/tmp/ws");
        let dirs = layout.required_dirs();
        assert!(
            dirs.contains(&PathBuf::from("/tmp/ws")),
            "root should be included"
        );
        assert!(
            dirs.contains(&PathBuf::from("/tmp/ws/inbox")),
            "inbox should be included"
        );
        assert!(
            dirs.contains(&PathBuf::from("/tmp/ws/archive/inbox")),
            "inbox archive should be included"
        );
        assert!(
            dirs.contains(&PathBuf::from("/tmp/ws/config")),
            "config should be included"
        );
        for dir in &dirs {
            assert!(
                dir.starts_with("/tmp/ws"),
                "required dir should be under root: {}",
                dir.display()
            );
        }
    }
}
