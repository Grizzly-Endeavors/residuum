//! Workspace directory layout and path helpers.

use std::path::{Path, PathBuf};

/// The workbench directory's name inside the workspace, which is also its
/// workspace-relative path.
pub const WORKBENCH_DIR: &str = "workbench";

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

    /// Path to the knowledge wiki -- an OKF bundle of agent-maintained concept pages.
    #[must_use]
    pub fn wiki_dir(&self) -> PathBuf {
        self.root.join("wiki")
    }

    /// Path to the wiki's root `index.md` -- the catalog injected into every prompt.
    #[must_use]
    pub fn wiki_index_md(&self) -> PathBuf {
        self.root.join("wiki/index.md")
    }

    /// Path to the wiki's root `log.md` -- append-only history of wiki changes.
    #[must_use]
    pub fn wiki_log_md(&self) -> PathBuf {
        self.root.join("wiki/log.md")
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

    /// Path to the durable record of which session run ids have already been
    /// merged into an episode, keyed by run id.
    #[must_use]
    pub fn merged_runs_json(&self) -> PathBuf {
        self.root.join("memory/merged_runs.json")
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

    /// Path to the workbench directory: the artifacts the agent builds for the
    /// user (single pages or folders), served in the web UI at `/workbench/{name}`.
    #[must_use]
    pub fn workbench_dir(&self) -> PathBuf {
        self.root.join(WORKBENCH_DIR)
    }

    /// Path to BOOTSTRAP.md -- first-run guidance, deleted after first conversation.
    #[must_use]
    pub fn bootstrap_md(&self) -> PathBuf {
        self.root.join("BOOTSTRAP.md")
    }

    /// Path to the agent inbox directory for background tasks and notifications.
    #[must_use]
    pub fn agent_inbox_dir(&self) -> PathBuf {
        self.root.join("inbox/agent")
    }

    /// Path to the user inbox directory for user-facing items.
    #[must_use]
    pub fn user_inbox_dir(&self) -> PathBuf {
        self.root.join("inbox/user")
    }

    /// Path to the agent inbox archive directory.
    #[must_use]
    pub fn agent_inbox_archive_dir(&self) -> PathBuf {
        self.root.join("archive/inbox/agent")
    }

    /// Path to the user inbox archive directory.
    #[must_use]
    pub fn user_inbox_archive_dir(&self) -> PathBuf {
        self.root.join("archive/inbox/user")
    }

    /// Path to the directory holding copied attachment files for user inbox items,
    /// one subdirectory per item ID.
    #[must_use]
    pub fn user_inbox_attachments_dir(&self) -> PathBuf {
        self.root.join("inbox/user/attachments")
    }

    /// Path to the archived counterpart of `user_inbox_attachments_dir` — where an
    /// item's attachment subdirectory moves to when the item itself is archived.
    #[must_use]
    pub fn user_inbox_archive_attachments_dir(&self) -> PathBuf {
        self.root.join("archive/inbox/user/attachments")
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

    /// Path to SUBCONSCIOUS.md -- check policy for the subconscious classifier.
    #[must_use]
    pub fn subconscious_md(&self) -> PathBuf {
        self.root.join("SUBCONSCIOUS.md")
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

    /// Path to `config/agent-card.json` — the A2A agent card: what this
    /// agent advertises to other agents that reach it over A2A.
    #[must_use]
    pub fn agent_card_json(&self) -> PathBuf {
        self.root.join("config/agent-card.json")
    }

    /// Path to the session store directory: per-run metadata and transcripts,
    /// organized by date.
    ///
    /// Created on-demand when the first run is recorded, not at bootstrap.
    #[must_use]
    pub fn sessions_dir(&self) -> PathBuf {
        self.root.join("memory/sessions")
    }

    /// Path to `memory/sessions/resume_points.json` -- persisted resume
    /// points, keyed by session address, loaded when the session registry is
    /// constructed at startup.
    #[must_use]
    pub fn resume_points_json(&self) -> PathBuf {
        self.sessions_dir().join("resume_points.json")
    }

    /// Path to `pulse_state.json` -- persisted pulse scheduler state (`last_run`).
    #[must_use]
    pub fn pulse_state_json(&self) -> PathBuf {
        self.root.join("pulse_state.json")
    }

    /// Path to `teams_state.json` -- the Teams owner and known conversation references.
    #[must_use]
    pub fn teams_state_json(&self) -> PathBuf {
        self.root.join("teams_state.json")
    }

    /// Path to `discord_state.json` -- the Discord owner and known conversations.
    #[must_use]
    pub fn discord_state_json(&self) -> PathBuf {
        self.root.join("discord_state.json")
    }

    /// Path to `telegram_state.json` -- the Telegram owner and known conversations.
    #[must_use]
    pub fn telegram_state_json(&self) -> PathBuf {
        self.root.join("telegram_state.json")
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
            self.wiki_dir(),
            self.memory_dir(),
            self.episodes_dir(),
            self.search_index_dir(),
            self.skills_dir(),
            self.workbench_dir(),
            self.agent_inbox_dir(),
            self.user_inbox_dir(),
            self.agent_inbox_archive_dir(),
            self.user_inbox_archive_dir(),
            self.user_inbox_attachments_dir(),
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
            layout.agent_inbox_dir(),
            PathBuf::from("/tmp/ws/inbox/agent"),
            "agent_inbox_dir path"
        );
        assert_eq!(
            layout.user_inbox_dir(),
            PathBuf::from("/tmp/ws/inbox/user"),
            "user_inbox_dir path"
        );
        assert_eq!(
            layout.agent_inbox_archive_dir(),
            PathBuf::from("/tmp/ws/archive/inbox/agent"),
            "agent_inbox_archive_dir path"
        );
        assert_eq!(
            layout.user_inbox_archive_dir(),
            PathBuf::from("/tmp/ws/archive/inbox/user"),
            "user_inbox_archive_dir path"
        );
        assert_eq!(
            layout.user_inbox_attachments_dir(),
            PathBuf::from("/tmp/ws/inbox/user/attachments"),
            "user_inbox_attachments_dir path"
        );
        assert_eq!(
            layout.user_inbox_archive_attachments_dir(),
            PathBuf::from("/tmp/ws/archive/inbox/user/attachments"),
            "user_inbox_archive_attachments_dir path"
        );
        assert_eq!(
            layout.vectors_db(),
            PathBuf::from("/tmp/ws/memory/vectors.db"),
            "vectors_db path"
        );
        assert_eq!(
            layout.bootstrap_md(),
            PathBuf::from("/tmp/ws/BOOTSTRAP.md"),
            "bootstrap_md path"
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
            layout.agent_card_json(),
            PathBuf::from("/tmp/ws/config/agent-card.json"),
            "agent_card_json path"
        );
        assert_eq!(
            layout.subconscious_md(),
            PathBuf::from("/tmp/ws/SUBCONSCIOUS.md"),
            "subconscious_md path"
        );
    }

    #[test]
    fn layout_wiki_paths() {
        let layout = WorkspaceLayout::new("/tmp/ws");
        assert_eq!(
            layout.wiki_dir(),
            PathBuf::from("/tmp/ws/wiki"),
            "wiki_dir path"
        );
        assert_eq!(
            layout.wiki_index_md(),
            PathBuf::from("/tmp/ws/wiki/index.md"),
            "wiki_index_md path"
        );
        assert_eq!(
            layout.wiki_log_md(),
            PathBuf::from("/tmp/ws/wiki/log.md"),
            "wiki_log_md path"
        );
        assert!(
            layout.required_dirs().contains(&layout.wiki_dir()),
            "wiki dir should be a required dir"
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
            layout.pulse_state_json(),
            PathBuf::from("/tmp/ws/pulse_state.json"),
            "pulse_state_json path"
        );
        assert_eq!(
            layout.scheduled_actions_json(),
            PathBuf::from("/tmp/ws/scheduled_actions.json"),
            "scheduled_actions_json path"
        );
        assert_eq!(
            layout.resume_points_json(),
            PathBuf::from("/tmp/ws/memory/sessions/resume_points.json"),
            "resume_points_json path"
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
            dirs.contains(&PathBuf::from("/tmp/ws/inbox/agent")),
            "agent inbox should be included"
        );
        assert!(
            dirs.contains(&PathBuf::from("/tmp/ws/inbox/user")),
            "user inbox should be included"
        );
        assert!(
            dirs.contains(&PathBuf::from("/tmp/ws/archive/inbox/agent")),
            "agent inbox archive should be included"
        );
        assert!(
            dirs.contains(&PathBuf::from("/tmp/ws/archive/inbox/user")),
            "user inbox archive should be included"
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
