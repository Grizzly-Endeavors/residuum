//! Skill management tools: activate and deactivate skills.

use async_trait::async_trait;
use serde_json::Value;

use crate::models::ToolDefinition;
use crate::skills::SharedSkillState;

use super::{Tool, ToolError, ToolResult};

// ─── skill_activate ──────────────────────────────────────────────────────────

/// Tool for activating a skill's instructions into the system prompt.
pub struct SkillActivateTool {
    state: SharedSkillState,
}

impl SkillActivateTool {
    /// Create a new `SkillActivateTool`.
    #[must_use]
    pub fn new(state: SharedSkillState) -> Self {
        Self { state }
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

        let mut state = self.state.lock().await;
        match state.activate(name).await {
            Ok(active) => Ok(ToolResult::success(format!(
                "Activated skill '{}'.",
                active.name
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
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use crate::skills::{SkillIndex, SkillState};
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

        let index = SkillIndex::scan(&[dir.path().to_path_buf()], None)
            .await
            .unwrap();
        let state = Arc::new(tokio::sync::Mutex::new(SkillState::new(
            index,
            vec![dir.path().to_path_buf()],
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
}
