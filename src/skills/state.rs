use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;

use super::{index::SkillIndex, parser::parse_skill_md, types::ActiveSkill};

/// Shared skill state, guarded by a mutex for concurrent access.
pub type SharedSkillState = Arc<tokio::sync::Mutex<SkillState>>;

/// Skill state manager: holds the index and active skills.
pub struct SkillState {
    index: SkillIndex,
    active: Vec<ActiveSkill>,
    dirs: Vec<PathBuf>,
}

impl SkillState {
    /// Create a new skill state with a pre-built index.
    #[must_use]
    pub fn new(index: SkillIndex, dirs: Vec<PathBuf>) -> Self {
        Self {
            index,
            active: Vec::new(),
            dirs,
        }
    }

    /// Create a new shared skill state.
    #[must_use]
    pub fn new_shared(index: SkillIndex, dirs: Vec<PathBuf>) -> SharedSkillState {
        Arc::new(tokio::sync::Mutex::new(Self::new(index, dirs)))
    }

    /// Activate a skill by name.
    ///
    /// Reads the full `SKILL.md`, parses the body, and adds it to the active list.
    ///
    /// # Errors
    /// Returns an error if the skill is not found, already active, or the
    /// file cannot be read.
    #[tracing::instrument(skip_all, fields(name = %name))]
    pub async fn activate(&mut self, name: &str) -> anyhow::Result<&ActiveSkill> {
        if self
            .active
            .iter()
            .any(|a| a.name.eq_ignore_ascii_case(name))
        {
            tracing::debug!("skill already active");
            anyhow::bail!("skill '{name}' is already active");
        }

        let entry = self.index.find_by_name(name).ok_or_else(|| {
            tracing::debug!("skill not found in index");
            anyhow::anyhow!("skill '{name}' not found")
        })?;

        let skill_md_path = entry.skill_dir.join("SKILL.md");
        let file_content = tokio::fs::read_to_string(&skill_md_path)
            .await
            .with_context(|| format!("failed to read SKILL.md for '{}'", entry.name))?;

        let (_fm, body) = parse_skill_md(&file_content)
            .inspect_err(|e| tracing::error!(path = %skill_md_path.display(), error = %e, "failed to parse SKILL.md at activation time"))?;

        let idx = self.active.len();
        self.active.push(ActiveSkill {
            name: entry.name.clone(),
            body,
            skill_dir: entry.skill_dir.clone(),
        });

        tracing::info!("skill activated");
        self.active
            .get(idx)
            .ok_or_else(|| anyhow::anyhow!("active vec empty after push"))
    }

    /// Deactivate a skill by name.
    ///
    /// # Errors
    /// Returns an error if the skill is not currently active.
    #[tracing::instrument(skip_all, fields(name = %name))]
    pub fn deactivate(&mut self, name: &str) -> anyhow::Result<()> {
        let pos = self
            .active
            .iter()
            .position(|a| a.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| anyhow::anyhow!("skill '{name}' is not currently active"))?;

        self.active.remove(pos);
        tracing::info!(name = %name, "skill deactivated");
        Ok(())
    }

    /// Rescan skill directories to rebuild the index.
    ///
    /// Removes any active skills whose names no longer appear in the new index.
    /// For an active skill whose name still resolves but whose backing source
    /// directory changed (e.g. a workspace skill now shadows a user-global
    /// skill of the same name), refreshes its body from the new source, or
    /// deactivates it with a warning if the new source can't be loaded.
    ///
    /// # Errors
    /// Returns an error if scanning fails.
    #[tracing::instrument(skip_all, fields(dirs = self.dirs.len()))]
    pub async fn rescan(&mut self) -> anyhow::Result<()> {
        let skills_before = self.index.entries().len();
        tracing::info!(
            skills_before = skills_before,
            "rescanning skill directories"
        );
        self.index = SkillIndex::scan(&self.dirs).await?;
        tracing::info!(
            skills_before = skills_before,
            skills_after = self.index.entries().len(),
            "skill rescan complete"
        );

        // Reconcile active skills against the new index. A name surviving the
        // rescan is not enough on its own: the *same name* can now resolve to
        // a different physical skill (e.g. a workspace `skills/notes/` now
        // shadows what used to be a user-global `notes` skill). An
        // already-active skill's body was captured at activation time, so if
        // we only checked the name we'd keep serving stale instructions under
        // a name the index now attributes to a different source, with no
        // signal to the agent that anything changed. Refresh from the new
        // source when the backing directory changed; if the new source can't
        // be loaded, deactivate rather than silently keep the stale copy.
        let previously_active = std::mem::take(&mut self.active);
        let mut still_active = Vec::with_capacity(previously_active.len());
        for active_skill in previously_active {
            let Some(entry) = self.index.find_by_name(&active_skill.name) else {
                tracing::warn!(name = %active_skill.name, "deactivating skill: no longer found after rescan");
                continue;
            };

            if entry.skill_dir == active_skill.skill_dir {
                still_active.push(active_skill);
                continue;
            }

            let skill_md_path = entry.skill_dir.join("SKILL.md");
            match tokio::fs::read_to_string(&skill_md_path)
                .await
                .with_context(|| format!("failed to read SKILL.md for '{}'", entry.name))
                .and_then(|content| parse_skill_md(&content))
            {
                Ok((_fm, body)) => {
                    tracing::warn!(
                        name = %active_skill.name,
                        old_source = %active_skill.skill_dir.display(),
                        new_source = %entry.skill_dir.display(),
                        "active skill's backing source changed after rescan; refreshed body from new source"
                    );
                    still_active.push(ActiveSkill {
                        name: entry.name.clone(),
                        body,
                        skill_dir: entry.skill_dir.clone(),
                    });
                }
                Err(e) => {
                    tracing::warn!(
                        name = %active_skill.name,
                        old_source = %active_skill.skill_dir.display(),
                        new_source = %entry.skill_dir.display(),
                        error = %e,
                        "deactivating skill: backing source changed after rescan and new source failed to load"
                    );
                }
            }
        }
        self.active = still_active;

        Ok(())
    }

    /// Format the skill index for the system prompt.
    #[must_use]
    pub fn format_index_for_prompt(&self) -> String {
        self.index.format_for_prompt()
    }

    /// Format active skill instructions for the system prompt.
    ///
    /// Returns `None` if no skills are active.
    #[must_use]
    pub fn format_active_for_prompt(&self) -> Option<String> {
        if self.active.is_empty() {
            return None;
        }

        let parts: Vec<String> = self
            .active
            .iter()
            .map(|skill| {
                format!(
                    "<active_skill name=\"{}\">\n{}\n</active_skill>",
                    skill.name, skill.body
                )
            })
            .collect();

        Some(parts.join("\n\n"))
    }

    /// Get the names of all active skills.
    #[must_use]
    pub fn active_skill_names(&self) -> Vec<&str> {
        self.active.iter().map(|a| a.name.as_str()).collect()
    }

    /// Get a reference to the current skill index.
    #[must_use]
    pub fn index(&self) -> &SkillIndex {
        &self.index
    }

    /// Get the skill scan directories (used when building isolated subagent state).
    #[must_use]
    pub fn dirs(&self) -> &[PathBuf] {
        &self.dirs
    }
}

#[cfg(test)]
mod tests {
    use super::super::index::SkillIndex;
    use super::SkillState;

    // ── SkillState ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn activate_and_deactivate() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("test-skill");
        tokio::fs::create_dir(&skill_dir).await.unwrap();
        tokio::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: test-skill\ndescription: \"Test\"\n---\n\nSkill body here.\n",
        )
        .await
        .unwrap();

        let index = SkillIndex::scan(&[dir.path().to_path_buf()]).await.unwrap();
        let mut state = SkillState::new(index, vec![dir.path().to_path_buf()]);

        assert!(
            state.active_skill_names().is_empty(),
            "should start with no active skills"
        );

        let active = state.activate("test-skill").await.unwrap();
        assert_eq!(active.name, "test-skill");
        assert!(active.body.contains("Skill body here"));
        assert_eq!(state.active_skill_names(), vec!["test-skill"]);

        state.deactivate("test-skill").unwrap();
        assert!(
            state.active_skill_names().is_empty(),
            "should have no active skills after deactivation"
        );
    }

    #[tokio::test]
    async fn activate_nonexistent() {
        let index = SkillIndex::default();
        let mut state = SkillState::new(index, vec![]);

        let result = state.activate("nonexistent").await;
        assert!(result.is_err(), "should error for nonexistent skill");
    }

    #[tokio::test]
    async fn activate_already_active() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("test-skill");
        tokio::fs::create_dir(&skill_dir).await.unwrap();
        tokio::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: test-skill\ndescription: \"Test\"\n---\n",
        )
        .await
        .unwrap();

        let index = SkillIndex::scan(&[dir.path().to_path_buf()]).await.unwrap();
        let mut state = SkillState::new(index, vec![dir.path().to_path_buf()]);

        state.activate("test-skill").await.unwrap();
        let result = state.activate("test-skill").await;
        assert!(result.is_err(), "should error for already active skill");
    }

    #[test]
    fn deactivate_not_active() {
        let index = SkillIndex::default();
        let mut state = SkillState::new(index, vec![]);

        let result = state.deactivate("nonexistent");
        assert!(result.is_err(), "should error for inactive skill");
    }

    #[tokio::test]
    async fn rescan_removes_stale_active() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("test-skill");
        tokio::fs::create_dir(&skill_dir).await.unwrap();
        tokio::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: test-skill\ndescription: \"Test\"\n---\n\nBody.\n",
        )
        .await
        .unwrap();

        let index = SkillIndex::scan(&[dir.path().to_path_buf()]).await.unwrap();
        let mut state = SkillState::new(index, vec![dir.path().to_path_buf()]);

        state.activate("test-skill").await.unwrap();
        assert_eq!(state.active_skill_names().len(), 1);

        // Remove the skill directory
        tokio::fs::remove_dir_all(&skill_dir).await.unwrap();

        state.rescan().await.unwrap();
        assert!(
            state.active_skill_names().is_empty(),
            "stale active skill should be removed after rescan"
        );
    }

    #[tokio::test]
    async fn rescan_preserves_still_valid_active() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("test-skill");
        tokio::fs::create_dir(&skill_dir).await.unwrap();
        tokio::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: test-skill\ndescription: \"Test\"\n---\n\nBody.\n",
        )
        .await
        .unwrap();

        let index = SkillIndex::scan(&[dir.path().to_path_buf()]).await.unwrap();
        let mut state = SkillState::new(index, vec![dir.path().to_path_buf()]);

        state.activate("test-skill").await.unwrap();
        state.rescan().await.unwrap();
        assert_eq!(
            state.active_skill_names(),
            vec!["test-skill"],
            "active skill should remain after rescan when dir is unchanged"
        );
    }

    #[tokio::test]
    async fn rescan_refreshes_active_skill_when_source_changes() {
        let ws_dir = tempfile::tempdir().unwrap();
        let user_dir = tempfile::tempdir().unwrap();

        // User-global skill named "notes" is active.
        let user_skill = user_dir.path().join("notes");
        tokio::fs::create_dir(&user_skill).await.unwrap();
        tokio::fs::write(
            user_skill.join("SKILL.md"),
            "---\nname: notes\ndescription: \"User-global notes\"\n---\n\nUser-global body.\n",
        )
        .await
        .unwrap();

        let dirs = vec![ws_dir.path().to_path_buf(), user_dir.path().to_path_buf()];
        let index = SkillIndex::scan(&dirs).await.unwrap();
        let mut state = SkillState::new(index, dirs);

        let active = state.activate("notes").await.unwrap();
        assert!(active.body.contains("User-global body."));

        // The workspace defining its own higher-priority "notes" skill becomes
        // active (mirrors a workspace file being added, then `rescan` running).
        let ws_skill = ws_dir.path().join("notes");
        tokio::fs::create_dir(&ws_skill).await.unwrap();
        tokio::fs::write(
            ws_skill.join("SKILL.md"),
            "---\nname: notes\ndescription: \"Workspace notes\"\n---\n\nWorkspace body.\n",
        )
        .await
        .unwrap();

        state.rescan().await.unwrap();

        // The index now attributes "notes" to the workspace skill. The
        // already-active skill must not keep silently serving the old
        // user-global body under that name: it must either be refreshed to
        // reflect the new (workspace) source, or deactivated outright.
        match state.active_skill_names().as_slice() {
            [] => {
                // Deactivating instead of refreshing is an acceptable, non-stale outcome.
            }
            ["notes"] => {
                let prompt = state.format_active_for_prompt().unwrap();
                assert!(
                    prompt.contains("Workspace body."),
                    "active skill should reflect the new higher-priority source"
                );
                assert!(
                    !prompt.contains("User-global body."),
                    "must not keep silently serving the stale user-global body"
                );
            }
            other => panic!("unexpected active skills after rescan: {other:?}"),
        }
    }

    #[tokio::test]
    async fn deactivate_case_insensitive() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("test-skill");
        tokio::fs::create_dir(&skill_dir).await.unwrap();
        tokio::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: test-skill\ndescription: \"Test\"\n---\n\nBody.\n",
        )
        .await
        .unwrap();

        let index = SkillIndex::scan(&[dir.path().to_path_buf()]).await.unwrap();
        let mut state = SkillState::new(index, vec![dir.path().to_path_buf()]);

        state.activate("test-skill").await.unwrap();
        state.deactivate("TEST-SKILL").unwrap();
        assert!(state.active_skill_names().is_empty());
    }

    #[test]
    fn format_active_none_when_empty() {
        let state = SkillState::new(SkillIndex::default(), vec![]);
        assert!(
            state.format_active_for_prompt().is_none(),
            "should return None when no skills active"
        );
    }

    #[tokio::test]
    async fn format_active_with_skills() {
        let dir = tempfile::tempdir().unwrap();

        let skill1 = dir.path().join("skill-a");
        tokio::fs::create_dir(&skill1).await.unwrap();
        tokio::fs::write(
            skill1.join("SKILL.md"),
            "---\nname: skill-a\ndescription: \"Skill A\"\n---\n\nBody A.\n",
        )
        .await
        .unwrap();

        let skill2 = dir.path().join("skill-b");
        tokio::fs::create_dir(&skill2).await.unwrap();
        tokio::fs::write(
            skill2.join("SKILL.md"),
            "---\nname: skill-b\ndescription: \"Skill B\"\n---\n\nBody B.\n",
        )
        .await
        .unwrap();

        let index = SkillIndex::scan(&[dir.path().to_path_buf()]).await.unwrap();
        let mut state = SkillState::new(index, vec![dir.path().to_path_buf()]);

        state.activate("skill-a").await.unwrap();
        state.activate("skill-b").await.unwrap();

        let output = state.format_active_for_prompt().unwrap();
        assert!(
            output.contains("<active_skill name=\"skill-a\">"),
            "should contain skill-a tag"
        );
        assert!(output.contains("Body A."), "should contain skill-a body");
        assert!(
            output.contains("<active_skill name=\"skill-b\">"),
            "should contain skill-b tag"
        );
        assert!(output.contains("Body B."), "should contain skill-b body");
    }
}
