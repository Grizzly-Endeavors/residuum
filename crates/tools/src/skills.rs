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
        // without a config reload. The lock is released before publishing.
        let (activation, report) = {
            let mut state = self.state.lock().await;
            let report = match state.rescan().await {
                Ok(report) => report,
                Err(e) => {
                    return Ok(ToolResult::error(format!("failed to rescan skills: {e}")));
                }
            };
            let activation = state.activate(name).await.map(|active| active.name.clone());
            (activation, report)
        };

        // A dropped skill is new by construction: the rescan removes it from
        // the active list. A notice is published only when the rescan reports
        // it as new, so one that persists is not repeated on every activation.
        for message in report.dropped.iter().chain(&report.new_notices) {
            publish_notice(self.publisher.as_ref(), message.clone()).await;
        }

        // The agent is the one relying on a dropped skill, so the result names
        // it. A failed activation also names the new notices, so an agent that
        // just wrote a skill learns why it is not loadable.
        let (is_error, mut output) = match activation {
            Ok(skill_name) => (false, format!("Activated skill '{skill_name}'.")),
            Err(e) => (true, e.to_string()),
        };
        for dropped in &report.dropped {
            output.push('\n');
            output.push_str(dropped);
        }
        if is_error {
            for notice in &report.new_notices {
                output.push('\n');
                output.push_str(notice);
            }
        }
        Ok(if is_error {
            ToolResult::error(output)
        } else {
            ToolResult::success(output)
        })
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
        use residuum_bus::{NoticeEvent, NotifyName, SYSTEM_CHANNEL, topics};

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

        let bus = residuum_bus::spawn_broker();
        let mut notices: residuum_bus::Subscriber<NoticeEvent> = bus
            .subscribe(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
            .await
            .unwrap();
        let tool = SkillActivateTool::new(Arc::clone(&state)).with_publisher(bus.publisher());

        let result = tool
            .execute(serde_json::json!({"name": "kept-skill"}))
            .await
            .unwrap();
        assert!(!result.is_error, "{}", result.output);
        assert_eq!(
            state.lock().await.active_skill_names(),
            vec!["kept-skill"],
            "the deleted skill should be dropped by the activation's rescan"
        );
        assert!(
            result.output.contains("gone-skill"),
            "the result should name the dropped skill: {}",
            result.output
        );

        let notice = notices
            .recv()
            .await
            .unwrap()
            .expect("a notice was published for the dropped skill");
        assert!(
            notice.message.contains("gone-skill"),
            "the published notice should name the dropped skill: {}",
            notice.message
        );
    }

    /// Publish a sentinel and return the notices received before it. The bus
    /// delivers one channel's messages in the order they were sent, so these
    /// are the notices published since the previous call.
    async fn notices_since_last_call(
        publisher: &residuum_bus::Publisher,
        notices: &mut residuum_bus::Subscriber<residuum_bus::NoticeEvent>,
    ) -> Vec<String> {
        const SENTINEL: &str = "sentinel";
        residuum_bus::notice::publish_notice(publisher, SENTINEL.to_string()).await;
        let mut messages = Vec::new();
        loop {
            let notice = notices
                .recv()
                .await
                .unwrap()
                .expect("the sentinel notice is delivered");
            if notice.message == SENTINEL {
                return messages;
            }
            messages.push(notice.message);
        }
    }

    #[tokio::test]
    async fn activate_publishes_a_notice_once_when_it_is_new() {
        use residuum_bus::{NoticeEvent, NotifyName, SYSTEM_CHANNEL, topics};

        let dir = tempfile::tempdir().unwrap();
        for name in ["first-skill", "second-skill", "third-skill"] {
            write_skill(
                dir.path(),
                name,
                &format!("---\nname: {name}\ndescription: \"Valid\"\n---\n\nBody.\n"),
            )
            .await;
        }
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
        let publisher = bus.publisher();
        let tool = SkillActivateTool::new(Arc::clone(&state)).with_publisher(bus.publisher());

        // The first rescan finds the broken skill for the first time.
        let activated = tool
            .execute(serde_json::json!({"name": "first-skill"}))
            .await
            .unwrap();
        assert!(!activated.is_error, "{}", activated.output);
        let first = notices_since_last_call(&publisher, &mut notices).await;
        assert_eq!(
            first.len(),
            1,
            "one notice for the new broken skill: {first:?}"
        );
        assert!(
            first.first().is_some_and(|m| m.contains("bad-skill")),
            "the notice should name the broken skill: {first:?}"
        );

        // A later rescan reports the same broken skill, which is not repeated.
        let second_activation = tool
            .execute(serde_json::json!({"name": "second-skill"}))
            .await
            .unwrap();
        assert!(!second_activation.is_error, "{}", second_activation.output);
        let second = notices_since_last_call(&publisher, &mut notices).await;
        assert!(
            second.is_empty(),
            "a notice that persists is not published again: {second:?}"
        );

        // A skill that breaks after startup is new, so it is published once.
        write_skill(dir.path(), "worse-skill", "---\n: broken [[\n---\n").await;
        let third_activation = tool
            .execute(serde_json::json!({"name": "third-skill"}))
            .await
            .unwrap();
        assert!(!third_activation.is_error, "{}", third_activation.output);
        let third = notices_since_last_call(&publisher, &mut notices).await;
        assert_eq!(third.len(), 1, "only the new broken skill: {third:?}");
        assert!(
            third.first().is_some_and(|m| m.contains("worse-skill")),
            "the notice should name the new broken skill: {third:?}"
        );
    }

    #[tokio::test]
    async fn failed_activation_names_the_skipped_skill_in_its_result() {
        let dir = tempfile::tempdir().unwrap();
        write_skill(dir.path(), "bad-skill", "---\n: invalid [[\n---\n").await;
        let state = Arc::new(tokio::sync::Mutex::new(SkillState::new(
            SkillIndex::default(),
            vec![SkillDir::agent(dir.path())],
        )));

        let result = SkillActivateTool::new(Arc::clone(&state))
            .execute(serde_json::json!({"name": "bad-skill"}))
            .await
            .unwrap();
        assert!(
            result.is_error,
            "a skipped skill is not active: {}",
            result.output
        );
        assert!(
            result.output.contains("skipped skill at"),
            "the failed activation should carry the rescan's notice: {}",
            result.output
        );
        assert!(
            result.output.contains("bad-skill"),
            "the notice should name the skipped skill's file: {}",
            result.output
        );
    }
}
