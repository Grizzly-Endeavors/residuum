use async_trait::async_trait;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use residuum_config::agent_keys::{Redactor, SharedAgentKeys};
use residuum_inference::{ToolDefinition, ToolError, ToolResult};

use super::auto_mode::SharedAutoMode;
use super::tool_result_store::ToolResultStore;

pub(crate) const CANCELLED_WHILE_RUNNING: &str =
    "cancelled: the turn was stopped while this tool was running";

pub(crate) const CANCELLED_BEFORE_START: &str =
    "cancelled: the turn was stopped before this tool call started";

/// Maximum length of a well-formed tool name.
///
/// Matches the `[a-zA-Z0-9_-]{1,64}` shape OpenAI-compatible APIs require of
/// function names, which every built-in and MCP tool name in this codebase
/// already satisfies.
const MAX_PLAUSIBLE_TOOL_NAME_LEN: usize = 64;

const TOOL_NAME_DISPLAY_LIMIT: usize = 80;

#[must_use]
pub(crate) fn is_plausible_tool_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_PLAUSIBLE_TOOL_NAME_LEN
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

#[must_use]
pub(crate) fn truncate_tool_name_for_display(name: &str) -> String {
    if name.chars().count() <= TOOL_NAME_DISPLAY_LIMIT {
        name.to_string()
    } else {
        let head: String = name.chars().take(TOOL_NAME_DISPLAY_LIMIT).collect();
        format!("{head}...")
    }
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;

    fn definition(&self) -> ToolDefinition;

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError>;

    async fn execute_cancellable(
        &self,
        arguments: Value,
        cancel: &CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        tokio::select! {
            biased;
            () = cancel.cancelled() => Ok(ToolResult::cancelled(CANCELLED_WHILE_RUNNING)),
            result = self.execute(arguments) => result,
        }
    }
}

pub struct ToolRegistry {
    tools: Vec<Box<dyn Tool>>,
    agent_keys: Option<SharedAgentKeys>,
    auto_mode: Option<SharedAutoMode>,
    result_store: Option<ToolResultStore>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self {
            tools: Vec::new(),
            agent_keys: None,
            auto_mode: None,
            result_store: None,
        }
    }

    pub fn set_auto_mode(&mut self, auto_mode: SharedAutoMode) {
        self.auto_mode = Some(auto_mode);
    }

    #[must_use]
    pub fn auto_mode(&self) -> Option<&SharedAutoMode> {
        self.auto_mode.as_ref()
    }

    pub fn set_result_store(&mut self, store: ToolResultStore) {
        self.result_store = Some(store);
    }

    #[must_use]
    pub fn result_store(&self) -> Option<&ToolResultStore> {
        self.result_store.as_ref()
    }

    pub fn set_agent_keys(&mut self, agent_keys: SharedAgentKeys) {
        self.agent_keys = Some(agent_keys);
    }

    #[must_use]
    pub fn agent_keys(&self) -> Option<&SharedAgentKeys> {
        self.agent_keys.as_ref()
    }

    pub async fn redactor(&self) -> Redactor {
        match &self.agent_keys {
            Some(keys) => keys.redactor().await,
            None => Redactor::default(),
        }
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        match self.tools.iter_mut().find(|t| t.name() == tool.name()) {
            Some(existing) => *existing = tool,
            None => self.tools.push(tool),
        }
    }

    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.tools.len();
        self.tools.retain(|t| t.name() != name);
        self.tools.len() != before
    }

    #[must_use]
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools.iter().map(|t| t.definition()).collect()
    }

    #[must_use]
    pub fn tool_names(&self) -> Vec<String> {
        self.tools.iter().map(|t| t.name().to_string()).collect()
    }

    #[tracing::instrument(skip_all, fields(tool.name = %name))]
    pub async fn execute(&self, name: &str, arguments: Value) -> Result<ToolResult, ToolError> {
        let tool = self
            .tools
            .iter()
            .find(|t| t.name() == name)
            .ok_or_else(|| ToolError::NotFound(name.to_string()))?;

        tracing::debug!("tool invocation");
        let result = tool.execute(arguments).await?;
        tracing::debug!(is_error = result.is_error, "tool result");
        Ok(result)
    }

    #[tracing::instrument(skip_all, fields(tool.name = %name))]
    pub async fn execute_cancellable(
        &self,
        name: &str,
        arguments: Value,
        cancel: &CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        let tool = self
            .tools
            .iter()
            .find(|t| t.name() == name)
            .ok_or_else(|| ToolError::NotFound(name.to_string()))?;

        tracing::debug!("tool invocation");
        let result = tool.execute_cancellable(arguments, cancel).await?;
        tracing::debug!(is_error = result.is_error, "tool result");
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use residuum_core::testing::clock;

    #[test]
    fn tool_result_cancelled_is_not_an_error() {
        let result = ToolResult::cancelled(CANCELLED_WHILE_RUNNING);
        assert!(
            !result.is_error,
            "a cancellation is not a failure and must not be flagged as one"
        );
        assert_eq!(result.output, CANCELLED_WHILE_RUNNING);
    }

    struct ForeverTool;

    #[async_trait]
    impl Tool for ForeverTool {
        fn name(&self) -> &'static str {
            "forever"
        }

        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                name: self.name().to_string(),
                description: String::new(),
                parameters: serde_json::json!({}),
            }
        }

        async fn execute(&self, _arguments: Value) -> Result<ToolResult, ToolError> {
            std::future::pending().await
        }
    }

    #[tokio::test(start_paused = true)]
    async fn default_execute_cancellable_drops_execute_on_cancellation() {
        let cancel = CancellationToken::new();
        cancel.cancel();

        let result = clock::within(
            std::time::Duration::from_secs(2),
            ForeverTool.execute_cancellable(serde_json::json!({}), &cancel),
        )
        .await
        .expect("an already-cancelled token must not wait for execute() at all")
        .expect("cancellation reports Ok, not a ToolError");

        assert!(!result.is_error, "cancellation is not a failure");
        assert_eq!(result.output, CANCELLED_WHILE_RUNNING);
    }

    #[tokio::test]
    async fn registry_not_found() {
        let registry = ToolRegistry::new();
        let result = registry.execute("nonexistent", Value::Null).await;
        assert!(result.is_err(), "should error on unknown tool");
        assert!(
            matches!(result.unwrap_err(), ToolError::NotFound(_)),
            "should be NotFound"
        );
    }

    #[test]
    fn registry_definitions_empty() {
        let registry = ToolRegistry::new();
        assert!(
            registry.definitions().is_empty(),
            "empty registry should have no definitions"
        );
    }

    struct NamedTool {
        name: &'static str,
        description: &'static str,
    }

    #[async_trait]
    impl Tool for NamedTool {
        fn name(&self) -> &'static str {
            self.name
        }

        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                name: self.name().to_string(),
                description: self.description.to_string(),
                parameters: serde_json::json!({}),
            }
        }

        async fn execute(&self, _arguments: Value) -> Result<ToolResult, ToolError> {
            Ok(ToolResult::success(self.description))
        }
    }

    #[test]
    fn registry_reregistering_a_tool_keeps_its_position() {
        let mut registry = ToolRegistry::new();
        for (name, description) in [("a", "first"), ("b", "second"), ("c", "third")] {
            registry.register(Box::new(NamedTool { name, description }));
        }

        registry.register(Box::new(NamedTool {
            name: "b",
            description: "replaced",
        }));

        let defs = registry.definitions();
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["a", "b", "c"], "a replaced tool keeps its slot");
        let replaced = defs.iter().find(|d| d.name == "b").unwrap();
        assert_eq!(replaced.description, "replaced", "the new definition wins");
    }

    #[test]
    fn registry_remove_then_register_appends_at_the_end() {
        let mut registry = ToolRegistry::new();
        for name in ["a", "b", "c"] {
            registry.register(Box::new(NamedTool {
                name,
                description: "",
            }));
        }

        assert!(registry.remove("a"), "a was registered");
        registry.register(Box::new(NamedTool {
            name: "a",
            description: "",
        }));

        let names = registry.tool_names();
        assert_eq!(
            names,
            ["b", "c", "a"],
            "removing and adding again is a new tool, so it goes last"
        );
    }

    #[test]
    fn plausible_tool_name_accepts_real_tool_names() {
        for name in [
            "write_file",
            "read_file",
            "skill_activate",
            "exec",
            "a",
            "a-b_c9",
        ] {
            assert!(is_plausible_tool_name(name), "'{name}' should be plausible");
        }
    }

    #[test]
    fn plausible_tool_name_rejects_leaked_glm_markup() {
        let name = "write_file\tcontent</arg_key><arg_value># Hello";
        assert!(
            !is_plausible_tool_name(name),
            "leaked markup should be rejected"
        );
    }

    #[test]
    fn plausible_tool_name_rejects_empty_and_oversized() {
        assert!(!is_plausible_tool_name(""), "empty name should be rejected");
        let too_long = "a".repeat(MAX_PLAUSIBLE_TOOL_NAME_LEN + 1);
        assert!(
            !is_plausible_tool_name(&too_long),
            "oversized name should be rejected"
        );
        let exactly_max = "a".repeat(MAX_PLAUSIBLE_TOOL_NAME_LEN);
        assert!(
            is_plausible_tool_name(&exactly_max),
            "max-length name should be accepted"
        );
    }

    #[test]
    fn plausible_tool_name_rejects_whitespace_and_angle_brackets() {
        for name in ["write file", "write\nfile", "<tool_call>", "write_file>"] {
            assert!(!is_plausible_tool_name(name), "'{name}' should be rejected");
        }
    }

    #[test]
    fn truncate_tool_name_leaves_short_names_untouched() {
        assert_eq!(truncate_tool_name_for_display("write_file"), "write_file");
    }

    #[test]
    fn truncate_tool_name_caps_long_names() {
        let raw = "x".repeat(200);
        let truncated = truncate_tool_name_for_display(&raw);
        assert_eq!(
            truncated.chars().count(),
            TOOL_NAME_DISPLAY_LIMIT + 3,
            "should keep the limit plus the ellipsis"
        );
        assert!(truncated.ends_with("..."), "should be marked as truncated");
    }
}
