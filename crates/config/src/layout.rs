use std::path::{Path, PathBuf};

use super::paths::{TeamPaths, team_dir};

#[derive(Debug, Clone)]
pub struct WorkspaceLayout {
    root: PathBuf,
    team: TeamPaths,
    #[cfg(any(test, feature = "test-support"))]
    tempdir_guard: Option<std::sync::Arc<tempfile::TempDir>>,
}

impl WorkspaceLayout {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root: PathBuf = root.into();
        let residuum_root = root.parent().unwrap_or(&root);
        let team = TeamPaths::new(team_dir(residuum_root));
        Self {
            root,
            team,
            #[cfg(any(test, feature = "test-support"))]
            tempdir_guard: None,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn with_tempdir_guard(mut self, dir: tempfile::TempDir) -> Self {
        self.tempdir_guard = Some(std::sync::Arc::new(dir));
        self
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn team(&self) -> &TeamPaths {
        &self.team
    }

    #[must_use]
    pub fn agent_name(&self) -> Option<&str> {
        self.root.file_name().and_then(|n| n.to_str())
    }

    #[must_use]
    pub fn soul_md(&self) -> PathBuf {
        self.root.join("SOUL.md")
    }

    #[must_use]
    pub fn memory_dir(&self) -> PathBuf {
        self.root.join("memory")
    }

    #[must_use]
    pub fn episodes_dir(&self) -> PathBuf {
        self.root.join("memory/episodes")
    }

    #[must_use]
    pub fn observations_json(&self) -> PathBuf {
        self.root.join("memory/observations.json")
    }

    #[must_use]
    pub fn merged_runs_json(&self) -> PathBuf {
        self.root.join("memory/merged_runs.json")
    }

    #[must_use]
    pub fn recent_messages_json(&self) -> PathBuf {
        self.root.join("memory/recent_messages.json")
    }

    #[must_use]
    pub fn recent_context_json(&self) -> PathBuf {
        self.root.join("memory/recent_context.json")
    }

    #[must_use]
    pub fn usage_totals_json(&self) -> PathBuf {
        self.root.join("memory/usage_totals.json")
    }

    #[must_use]
    pub fn context_breakdown_json(&self) -> PathBuf {
        self.root.join("memory/context_breakdown.json")
    }

    #[must_use]
    pub fn usage_records_dir(&self) -> PathBuf {
        self.root.join("memory/usage")
    }

    #[must_use]
    pub fn search_index_dir(&self) -> PathBuf {
        self.root.join("memory/.index")
    }

    #[must_use]
    pub fn index_manifest_json(&self) -> PathBuf {
        self.root.join("memory/.index_manifest.json")
    }

    #[must_use]
    pub fn vectors_db(&self) -> PathBuf {
        self.root.join("memory/vectors.db")
    }

    #[must_use]
    pub fn skills_dir(&self) -> PathBuf {
        self.root.join("skills")
    }

    #[must_use]
    pub fn bootstrap_md(&self) -> PathBuf {
        self.root.join("BOOTSTRAP.md")
    }

    #[must_use]
    pub fn agent_inbox_dir(&self) -> PathBuf {
        self.root.join("inbox/agent")
    }

    #[must_use]
    pub fn user_inbox_dir(&self) -> PathBuf {
        self.root.join("inbox/user")
    }

    #[must_use]
    pub fn agent_inbox_attachments_dir(&self) -> PathBuf {
        self.root.join("inbox/agent/attachments")
    }

    #[must_use]
    pub fn agent_inbox_archive_dir(&self) -> PathBuf {
        self.root.join("archive/inbox/agent")
    }

    #[must_use]
    pub fn agent_inbox_archive_attachments_dir(&self) -> PathBuf {
        self.root.join("archive/inbox/agent/attachments")
    }

    #[must_use]
    pub fn user_inbox_archive_dir(&self) -> PathBuf {
        self.root.join("archive/inbox/user")
    }

    #[must_use]
    pub fn user_inbox_attachments_dir(&self) -> PathBuf {
        self.root.join("inbox/user/attachments")
    }

    #[must_use]
    pub fn user_inbox_archive_attachments_dir(&self) -> PathBuf {
        self.root.join("archive/inbox/user/attachments")
    }

    #[must_use]
    pub fn observer_md(&self) -> PathBuf {
        self.root.join("memory/OBSERVER.md")
    }

    #[must_use]
    pub fn reflector_md(&self) -> PathBuf {
        self.root.join("memory/REFLECTOR.md")
    }

    #[must_use]
    pub fn heartbeat_yml(&self) -> PathBuf {
        self.root.join("HEARTBEAT.yml")
    }

    #[must_use]
    pub fn subconscious_md(&self) -> PathBuf {
        self.root.join("SUBCONSCIOUS.md")
    }

    #[must_use]
    pub fn config_dir(&self) -> PathBuf {
        self.root.join("config")
    }

    #[must_use]
    pub fn mcp_json(&self) -> PathBuf {
        self.root.join("config/mcp.json")
    }

    #[must_use]
    pub fn channels_toml(&self) -> PathBuf {
        self.root.join("config/channels.toml")
    }

    #[must_use]
    pub fn agent_card_json(&self) -> PathBuf {
        self.root.join("config/agent-card.json")
    }

    #[must_use]
    pub fn a2a_agents_json(&self) -> PathBuf {
        self.root.join("config/a2a.json")
    }

    #[must_use]
    pub fn a2a_dir(&self) -> PathBuf {
        self.root.join("a2a")
    }

    #[must_use]
    pub fn a2a_outbound_json(&self) -> PathBuf {
        self.a2a_dir().join("outbound.json")
    }

    #[must_use]
    pub fn a2a_tasks_dir(&self) -> PathBuf {
        self.a2a_dir().join("tasks")
    }

    #[must_use]
    pub fn sessions_dir(&self) -> PathBuf {
        self.root.join("memory/sessions")
    }

    #[must_use]
    pub fn resume_points_json(&self) -> PathBuf {
        self.sessions_dir().join("resume_points.json")
    }

    #[must_use]
    pub fn pulse_state_json(&self) -> PathBuf {
        self.root.join("pulse_state.json")
    }

    #[must_use]
    pub fn teams_state_json(&self) -> PathBuf {
        self.root.join("teams_state.json")
    }

    #[must_use]
    pub fn discord_state_json(&self) -> PathBuf {
        self.root.join("discord_state.json")
    }

    #[must_use]
    pub fn telegram_state_json(&self) -> PathBuf {
        self.root.join("telegram_state.json")
    }

    #[must_use]
    pub fn scheduled_actions_json(&self) -> PathBuf {
        self.root.join("scheduled_actions.json")
    }

    #[must_use]
    pub fn file_links_json(&self) -> PathBuf {
        self.root.join("file_links.json")
    }

    #[must_use]
    pub fn required_dirs(&self) -> Vec<PathBuf> {
        vec![
            self.root.clone(),
            self.memory_dir(),
            self.episodes_dir(),
            self.search_index_dir(),
            self.skills_dir(),
            self.agent_inbox_dir(),
            self.user_inbox_dir(),
            self.agent_inbox_archive_dir(),
            self.user_inbox_archive_dir(),
            self.agent_inbox_attachments_dir(),
            self.user_inbox_attachments_dir(),
            self.config_dir(),
            self.a2a_dir(),
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
            layout.agent_inbox_attachments_dir(),
            PathBuf::from("/tmp/ws/inbox/agent/attachments"),
            "agent_inbox_attachments_dir path"
        );
        assert_eq!(
            layout.agent_inbox_archive_attachments_dir(),
            PathBuf::from("/tmp/ws/archive/inbox/agent/attachments"),
            "agent_inbox_archive_attachments_dir path"
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
    fn layout_a2a_client_paths() {
        let layout = WorkspaceLayout::new("/tmp/ws");
        assert_eq!(
            layout.a2a_agents_json(),
            PathBuf::from("/tmp/ws/config/a2a.json"),
            "a2a_agents_json path"
        );
        assert_eq!(
            layout.a2a_dir(),
            PathBuf::from("/tmp/ws/a2a"),
            "a2a_dir path"
        );
        assert_eq!(
            layout.a2a_outbound_json(),
            PathBuf::from("/tmp/ws/a2a/outbound.json"),
            "a2a_outbound_json path"
        );
        assert!(
            layout.required_dirs().contains(&layout.a2a_dir()),
            "a2a dir should be a required dir"
        );
    }

    #[test]
    fn team_files_are_not_part_of_the_agent_layout() {
        let layout = WorkspaceLayout::new(Path::new("res").join("scout"));
        let team = layout.team();
        assert_eq!(team.root(), Path::new("res").join("team"));
        assert_eq!(
            team.wiki_index_md(),
            Path::new("res").join("team").join("wiki").join("index.md")
        );
        assert!(
            !layout
                .required_dirs()
                .iter()
                .any(|dir| dir.starts_with(team.root()) || dir.ends_with("wiki")),
            "the agent layout creates no team or wiki directories"
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
            layout.file_links_json(),
            PathBuf::from("/tmp/ws/file_links.json"),
            "file_links_json path"
        );
        assert_eq!(
            layout.resume_points_json(),
            PathBuf::from("/tmp/ws/memory/sessions/resume_points.json"),
            "resume_points_json path"
        );
    }

    #[test]
    fn layout_usage_totals_path() {
        let layout = WorkspaceLayout::new("/tmp/ws");
        assert_eq!(
            layout.usage_totals_json(),
            PathBuf::from("/tmp/ws/memory/usage_totals.json"),
            "usage_totals_json path"
        );
        assert_eq!(
            layout.usage_records_dir(),
            PathBuf::from("/tmp/ws/memory/usage"),
            "usage_records_dir path"
        );
        assert_eq!(
            layout.context_breakdown_json(),
            PathBuf::from("/tmp/ws/memory/context_breakdown.json"),
            "context_breakdown_json path"
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

    #[test]
    fn team_layer_sits_beside_the_agent_dir() {
        let layout = WorkspaceLayout::new(Path::new("res").join("scout"));
        assert_eq!(layout.team().root(), Path::new("res").join("team"));
        assert_eq!(layout.agent_name(), Some("scout"));
    }
}
