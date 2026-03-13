use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::{index::SkillIndex, parser::parse_skill_md, types::ActiveSkill};
use crate::error::ResiduumError;

/// Shared skill state, following the `SharedProjectState` pattern.
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
    /// Returns `ResiduumError::Skills` if the skill is not found, already active,
    /// or the file cannot be read.
    pub async fn activate(&mut self, name: &str) -> Result<&ActiveSkill, ResiduumError> {
        if self
            .active
            .iter()
            .any(|a| a.name.eq_ignore_ascii_case(name))
        {
            return Err(ResiduumError::Skills(format!(
                "skill '{name}' is already active"
            )));
        }

        let entry = self
            .index
            .find_by_name(name)
            .ok_or_else(|| ResiduumError::Skills(format!("skill '{name}' not found")))?;

        let skill_md_path = entry.skill_dir.join("SKILL.md");
        let file_content = tokio::fs::read_to_string(&skill_md_path)
            .await
            .map_err(|e| {
                ResiduumError::Skills(format!("failed to read SKILL.md for '{}': {e}", entry.name))
            })?;

        let (_fm, body) = parse_skill_md(&file_content).map_err(|e| {
            tracing::error!(path = %skill_md_path.display(), error = %e, "failed to parse SKILL.md at activation time");
            e
        })?;

        let skill_name = entry.name.clone();
        self.active.push(ActiveSkill {
            name: skill_name,
            body,
        });

        // Safe: we just pushed
        self.active.last().ok_or_else(|| {
            ResiduumError::Skills("unexpected: active skill not set after activation".into())
        })
    }

    /// Deactivate a skill by name.
    ///
    /// # Errors
    /// Returns `ResiduumError::Skills` if the skill is not currently active.
    pub fn deactivate(&mut self, name: &str) -> Result<(), ResiduumError> {
        let pos = self
            .active
            .iter()
            .position(|a| a.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| {
                ResiduumError::Skills(format!("skill '{name}' is not currently active"))
            })?;

        self.active.remove(pos);
        Ok(())
    }

    /// Rescan skill directories to rebuild the index.
    ///
    /// Removes any active skills whose names no longer appear in the new index.
    ///
    /// # Errors
    /// Returns `ResiduumError::Skills` if scanning fails.
    pub async fn rescan(&mut self, project_skills_dir: Option<&Path>) -> Result<(), ResiduumError> {
        self.index = SkillIndex::scan(&self.dirs, project_skills_dir).await?;

        // Remove active skills that no longer exist in the index
        for skill in &self.active {
            if self.index.find_by_name(&skill.name).is_none() {
                tracing::warn!(name = %skill.name, "deactivating skill: no longer found after rescan");
            }
        }
        self.active
            .retain(|a| self.index.find_by_name(&a.name).is_some());

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
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
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

        let index = SkillIndex::scan(&[dir.path().to_path_buf()], None)
            .await
            .unwrap();
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

        let index = SkillIndex::scan(&[dir.path().to_path_buf()], None)
            .await
            .unwrap();
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

        let index = SkillIndex::scan(&[dir.path().to_path_buf()], None)
            .await
            .unwrap();
        let mut state = SkillState::new(index, vec![dir.path().to_path_buf()]);

        state.activate("test-skill").await.unwrap();
        assert_eq!(state.active_skill_names().len(), 1);

        // Remove the skill directory
        tokio::fs::remove_dir_all(&skill_dir).await.unwrap();

        state.rescan(None).await.unwrap();
        assert!(
            state.active_skill_names().is_empty(),
            "stale active skill should be removed after rescan"
        );
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

        let index = SkillIndex::scan(&[dir.path().to_path_buf()], None)
            .await
            .unwrap();
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
