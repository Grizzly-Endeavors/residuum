//! Skill management tools: activate and deactivate skills.

use async_trait::async_trait;
use serde_json::Value;

use residuum_bus::Publisher;
use residuum_inference::ToolDefinition;
use residuum_skills::SharedSkillState;

use super::publish_notice;
use residuum_agent::Tool;
use residuum_inference::{ToolError, ToolResult};

// ─── skill_activate ──────────────────────────────────────────────────────────

/// Tool for activating a skill's instructions into the system prompt.
pub struct SkillActivateTool {
    state: SharedSkillState,
    publisher: Option<Publisher>,
}

impl SkillActivateTool {
    /// Create a new `SkillActivateTool`.
    #[must_use]
    pub fn new(state: SharedSkillState) -> Self {
        Self {
            state,
            publisher: None,
        }
    }

    /// Publish the notices each rescan produces, such as a skill that was
    /// skipped for invalid frontmatter.
    #[must_use]
    pub fn with_publisher(mut self, publisher: Publisher) -> Self {
        self.publisher = Some(publisher);
        self
    }
}

#[async_trait]
impl Tool for SkillActivateTool {
    fn name(&self) -> &'static str {
        "skill_activate"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: "Load a skill's full instructions into the system prompt. \
                Use when a task matches an available skill's description."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Name of the skill to activate (case-insensitive)"
                    }
                },
                "required": ["name"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let name = super::require_str(&arguments, "name")?;

        // Rescan before activating, so a skill authored since startup can be
        // activated and an active skill whose files were removed is dropped,
        // without a config reload. The notices go out after the lock is
        // released.
        let (activation, notices) = {
            let mut state = self.state.lock().await;
            let notices = match state.rescan().await {
                Ok(notices) => notices,
                Err(e) => {
                    return Ok(ToolResult::error(format!("failed to rescan skills: {e}")));
                }
            };
            let activation = state.activate(name).await.map(|active| active.name.clone());
            (activation, notices)
        };
        for notice in notices {
            publish_notice(self.publisher.as_ref(), notice).await;
        }
        match activation {
            Ok(skill_name) => Ok(ToolResult::success(format!(
                "Activated skill '{skill_name}'."
            ))),
            Err(e) => Ok(ToolResult::error(e.to_string())),
        }
    }
}

// ─── skill_deactivate ────────────────────────────────────────────────────────

/// Tool for deactivating a skill's instructions from the system prompt.
pub struct SkillDeactivateTool {
    state: SharedSkillState,
}

impl SkillDeactivateTool {
    /// Create a new `SkillDeactivateTool`.
    #[must_use]
    pub fn new(state: SharedSkillState) -> Self {
        Self { state }
    }
}

#[async_trait]
impl Tool for SkillDeactivateTool {
    fn name(&self) -> &'static str {
        "skill_deactivate"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description:
                "Remove a skill's instructions from the system prompt when no longer needed."
                    .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Name of the skill to deactivate"
                    }
                },
                "required": ["name"]
            }),
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let name = super::require_str(&arguments, "name")?;

        let mut state = self.state.lock().await;
        match state.deactivate(name) {
            Ok(()) => Ok(ToolResult::success(format!("Deactivated skill '{name}'."))),
            Err(e) => Ok(ToolResult::error(e.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use residuum_config::SkillDir;
    use residuum_skills::{SkillIndex, SkillState};
    use std::sync::Arc;

    #[test]
    fn tool_names() {
        let state = Arc::new(tokio::sync::Mutex::new(SkillState::new(
            SkillIndex::default(),
            vec![],
        )));

        assert_eq!(
            SkillActivateTool::new(Arc::clone(&state)).name(),
            "skill_activate",
            "activate tool name"
        );
        assert_eq!(
            SkillDeactivateTool::new(state).name(),
            "skill_deactivate",
            "deactivate tool name"
        );
    }

    #[tokio::test]
    async fn activate_nonexistent() {
        let state = Arc::new(tokio::sync::Mutex::new(SkillState::new(
            SkillIndex::default(),
            vec![],
        )));

        let tool = SkillActivateTool::new(state);
        let result = tool
            .execute(serde_json::json!({"name": "nonexistent"}))
            .await
            .unwrap();
        assert!(result.is_error, "should error for nonexistent skill");
    }

    #[tokio::test]
    async fn deactivate_not_active() {
        let state = Arc::new(tokio::sync::Mutex::new(SkillState::new(
            SkillIndex::default(),
            vec![],
        )));

        let tool = SkillDeactivateTool::new(state);
        let result = tool
            .execute(serde_json::json!({"name": "nonexistent"}))
            .await
            .unwrap();
        assert!(result.is_error, "should error for inactive skill");
    }

    #[tokio::test]
    async fn activate_and_deactivate_lifecycle() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join("test-skill");
        tokio::fs::create_dir(&skill_dir).await.unwrap();
        tokio::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: test-skill\ndescription: \"Test\"\n---\n\nBody.\n",
        )
        .await
        .unwrap();

        let index = SkillIndex::scan(&[residuum_config::SkillDir::agent(dir.path())])
            .await
            .unwrap();
        let state = Arc::new(tokio::sync::Mutex::new(SkillState::new(
            index,
            vec![SkillDir::agent(dir.path())],
        )));

        let activate = SkillActivateTool::new(Arc::clone(&state));
        let result = activate
            .execute(serde_json::json!({"name": "test-skill"}))
            .await
            .unwrap();
        assert!(!result.is_error, "activate should succeed");
        assert!(
            result.output.contains("test-skill"),
            "output should mention skill name"
        );

        // After activation, the skill body ("Body.") should be accessible
        let active_content = state.lock().await.format_active_for_prompt().unwrap();
        assert!(
            active_content.contains("Body."),
            "active skill content should contain SKILL.md body: {active_content}"
        );

        let deactivate = SkillDeactivateTool::new(Arc::clone(&state));
        let deactivate_result = deactivate
            .execute(serde_json::json!({"name": "test-skill"}))
            .await
            .unwrap();
        assert!(!deactivate_result.is_error, "deactivate should succeed");
    }

    async fn write_skill(root: &std::path::Path, name: &str, frontmatter: &str) {
        let skill_dir = root.join(name);
        tokio::fs::create_dir(&skill_dir).await.unwrap();
        tokio::fs::write(skill_dir.join("SKILL.md"), frontmatter)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn activate_finds_a_skill_created_after_startup() {
        let dir = tempfile::tempdir().unwrap();
        let state = Arc::new(tokio::sync::Mutex::new(SkillState::new(
            SkillIndex::default(),
            vec![SkillDir::agent(dir.path())],
        )));

        write_skill(
            dir.path(),
            "late-skill",
            "---\nname: late-skill\ndescription: \"Authored later\"\n---\n\nLate body.\n",
        )
        .await;

        let result = SkillActivateTool::new(Arc::clone(&state))
            .execute(serde_json::json!({"name": "late-skill"}))
            .await
            .unwrap();
        assert!(
            !result.is_error,
            "activate should rescan first: {}",
            result.output
        );
        assert_eq!(
            state.lock().await.active_skill_names(),
            vec!["late-skill"],
            "the rescanned skill should be active"
        );
    }

    #[tokio::test]
    async fn activate_drops_an_active_skill_whose_files_were_deleted() {
        let dir = tempfile::tempdir().unwrap();
        write_skill(
            dir.path(),
            "gone-skill",
            "---\nname: gone-skill\ndescription: \"Will be deleted\"\n---\n\nGone body.\n",
        )
        .await;
        write_skill(
            dir.path(),
            "kept-skill",
            "---\nname: kept-skill\ndescription: \"Stays\"\n---\n\nKept body.\n",
        )
        .await;
        let index = SkillIndex::scan(&[SkillDir::agent(dir.path())])
            .await
            .unwrap();
        let state = Arc::new(tokio::sync::Mutex::new(SkillState::new(
            index,
            vec![SkillDir::agent(dir.path())],
        )));
        SkillActivateTool::new(Arc::clone(&state))
            .execute(serde_json::json!({"name": "gone-skill"}))
            .await
            .unwrap();
        assert_eq!(state.lock().await.active_skill_names(), vec!["gone-skill"]);

        tokio::fs::remove_dir_all(dir.path().join("gone-skill"))
            .await
            .unwrap();

        let result = SkillActivateTool::new(Arc::clone(&state))
            .execute(serde_json::json!({"name": "kept-skill"}))
            .await
            .unwrap();
        assert!(!result.is_error, "{}", result.output);
        assert_eq!(
            state.lock().await.active_skill_names(),
            vec!["kept-skill"],
            "the deleted skill should be dropped by the activation's rescan"
        );
    }

    #[tokio::test]
    async fn activate_publishes_the_notices_its_rescan_produced() {
        use residuum_bus::{NoticeEvent, NotifyName, SYSTEM_CHANNEL, topics};

        let dir = tempfile::tempdir().unwrap();
        write_skill(
            dir.path(),
            "good-skill",
            "---\nname: good-skill\ndescription: \"Valid\"\n---\n\nGood body.\n",
        )
        .await;
        write_skill(dir.path(), "bad-skill", "---\n: invalid [[\n---\n").await;
        let state = Arc::new(tokio::sync::Mutex::new(SkillState::new(
            SkillIndex::default(),
            vec![SkillDir::agent(dir.path())],
        )));

        let bus = residuum_bus::spawn_broker();
        let mut notices: residuum_bus::Subscriber<NoticeEvent> = bus
            .subscribe(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
            .await
            .unwrap();
        let tool = SkillActivateTool::new(Arc::clone(&state)).with_publisher(bus.publisher());

        let result = tool
            .execute(serde_json::json!({"name": "good-skill"}))
            .await
            .unwrap();
        assert!(!result.is_error, "{}", result.output);

        let notice = notices
            .recv()
            .await
            .unwrap()
            .expect("a notice was published for the skipped skill");
        assert!(
            notice.message.contains("bad-skill"),
            "the notice should name the skipped skill: {}",
            notice.message
        );
    }
}
