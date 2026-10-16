//! The tool contract the turn loop dispatches through, and the registry that
//! holds a turn's tools.

use async_trait::async_trait;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::agent_keys::{Redactor, SharedAgentKeys};
use crate::inference::{ToolDefinition, ToolError, ToolResult};

use super::auto_mode::SharedAutoMode;

/// Reported when a tool call is interrupted mid-execution because the turn
/// was stopped — a user stop, a session's own stop, or daemon shutdown.
pub(crate) const CANCELLED_WHILE_RUNNING: &str =
    "cancelled: the turn was stopped while this tool was running";

/// Reported for a tool call in a batch that never ran because the turn had
/// already been stopped by the time its turn came up.
pub(crate) const CANCELLED_BEFORE_START: &str =
    "cancelled: the turn was stopped before this tool call started";

/// Maximum length of a well-formed tool name.
///
/// Matches the `[a-zA-Z0-9_-]{1,64}` shape OpenAI-compatible APIs require of
/// function names, which every built-in and MCP tool name in this codebase
/// already satisfies.
const MAX_PLAUSIBLE_TOOL_NAME_LEN: usize = 64;

/// Number of characters kept when a malformed tool name is logged or reported
/// back to the model, so a large blob of leaked markup doesn't flood the log
/// or the transcript.
const TOOL_NAME_DISPLAY_LIMIT: usize = 80;

/// Whether `name` could plausibly be a real tool name.
///
/// Every built-in and MCP tool name in this codebase is `[a-zA-Z0-9_-]`, at
/// most 64 characters. Anything else — whitespace, angle brackets, control
/// characters — means a model's tool-call name field was not cleanly parsed
/// by the inference provider, not that a genuinely unknown tool was
/// requested.
#[must_use]
pub(crate) fn is_plausible_tool_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_PLAUSIBLE_TOOL_NAME_LEN
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Truncate a raw, possibly-malformed tool name for safe logging or display.
#[must_use]
pub(crate) fn truncate_tool_name_for_display(name: &str) -> String {
    if name.chars().count() <= TOOL_NAME_DISPLAY_LIMIT {
        name.to_string()
    } else {
        let head: String = name.chars().take(TOOL_NAME_DISPLAY_LIMIT).collect();
        format!("{head}...")
    }
}

/// Trait for tool implementations that the agent can invoke.
#[async_trait]
pub trait Tool: Send + Sync {
    /// The unique name of this tool.
    fn name(&self) -> &'static str;

    /// The tool definition sent to the model.
    fn definition(&self) -> ToolDefinition;

    /// Execute the tool with the given arguments.
    ///
    /// # Errors
    /// Returns `ToolError` if the arguments are invalid or execution fails.
    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError>;

    /// Execute the tool, racing it against turn-level cancellation.
    ///
    /// `cancel` is cancelled when the turn this tool call belongs to is
    /// stopped — by the user, by a session's own stop, or by daemon
    /// shutdown. The default implementation drops `execute()`'s future
    /// outright the moment that happens and reports a generic cancellation
    /// result, which is correct for every tool that owns no external
    /// resource needing cleanup. `ExecTool` overrides this to kill its
    /// child process tree first and return whatever output it had already
    /// captured, instead of discarding it.
    ///
    /// # Errors
    /// Returns `ToolError` if the arguments are invalid or execution fails.
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

/// The tools a turn can call, with what the turn loop applies around every
/// call: the agent-key redactor and the Auto Mode check.
pub struct ToolRegistry {
    tools: Vec<Box<dyn Tool>>,
    /// The agent key store: the source of the redactor applied to every tool
    /// result, and what registration hands the `exec` tool.
    agent_keys: Option<SharedAgentKeys>,
    /// The agent's Auto Mode, checked before every tool call the turn loop
    /// dispatches. `None` means no call is checked.
    auto_mode: Option<SharedAutoMode>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolRegistry {
    /// Create a new empty tool registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            tools: Vec::new(),
            agent_keys: None,
            auto_mode: None,
        }
    }

    /// Attach the agent's Auto Mode, which the turn loop consults before
    /// dispatching each tool call.
    pub fn set_auto_mode(&mut self, auto_mode: SharedAutoMode) {
        self.auto_mode = Some(auto_mode);
    }

    /// The agent's Auto Mode, if one is attached.
    #[must_use]
    pub fn auto_mode(&self) -> Option<&SharedAutoMode> {
        self.auto_mode.as_ref()
    }

    /// Set the agent key store used to redact tool results.
    ///
    /// Call before registering the `exec` tool, which takes the store from
    /// here so it can expose and mint keys.
    pub fn set_agent_keys(&mut self, agent_keys: SharedAgentKeys) {
        self.agent_keys = Some(agent_keys);
    }

    /// The agent key store, if one is attached.
    #[must_use]
    pub fn agent_keys(&self) -> Option<&SharedAgentKeys> {
        self.agent_keys.as_ref()
    }

    /// The redactor for every current agent-key value; empty when no key
    /// store is attached.
    pub async fn redactor(&self) -> Redactor {
        match &self.agent_keys {
            Some(keys) => keys.redactor().await,
            None => Redactor::default(),
        }
    }

    /// Register a tool in the registry.
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.push(tool);
    }

    /// Remove a registered tool by name, if present.
    ///
    /// Used to reload a single conditionally-registered tool in place on a
    /// live registry (e.g. `ollama_web_search` after a config reload changes
    /// the standalone web search backend) without rebuilding the whole
    /// registry. Returns `true` if a tool was removed.
    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.tools.len();
        self.tools.retain(|t| t.name() != name);
        self.tools.len() != before
    }

    /// Get tool definitions for sending to the model.
    #[must_use]
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools.iter().map(|t| t.definition()).collect()
    }

    /// Names of all registered tools.
    ///
    /// Used to reserve the built-in tool namespace in the MCP registry so a
    /// colliding MCP tool is shadowed visibly rather than silently.
    #[must_use]
    pub fn tool_names(&self) -> Vec<String> {
        self.tools.iter().map(|t| t.name().to_string()).collect()
    }

    /// Execute a tool by name with the given arguments.
    ///
    /// # Errors
    /// Returns `ToolError::NotFound` if no tool with the given name exists,
    /// or propagates execution errors from the tool.
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

    /// Execute a tool by name, racing it against turn-level cancellation.
    ///
    /// See [`Tool::execute_cancellable`] for what happens when `cancel`
    /// fires while the tool is running.
    ///
    /// # Errors
    /// Returns `ToolError::NotFound` if no tool with the given name exists,
    /// or propagates execution errors from the tool.
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

    /// A tool whose `execute()` blocks forever, so the default
    /// `execute_cancellable()` can only ever return via the cancellation
    /// branch — proving it actually races rather than always awaiting
    /// `execute()` to completion.
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
        // Mirrors the malformed name Fireworks/vLLM have shipped for GLM tool
        // calls when the model's native `<arg_key>`/`<arg_value>` template
        // syntax isn't cleanly parsed into structured `tool_calls`.
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
