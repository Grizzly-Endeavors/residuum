//! Test tools for driving the turn loop without the real tool set, and the
//! seam that renders a model request without a provider call.
//!
//! Each tool has a fixed name and an empty parameter schema; a test's
//! scripted model calls it by that name.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use residuum_core::testing::gate::{Gate, GateEntry};
use residuum_mcp::{McpRegistry, SharedMcpRegistry};
use residuum_workspace::identity::IdentityFiles;

use super::Tool;
use super::context::{MemoryContext, PromptContext, SkillsContext};
use super::recent_messages::RecentMessages;
use super::tool::ToolRegistry;
use super::turn::{ModelRequest, build_model_request};
use residuum_inference::{CompletionOptions, ToolDefinition, ToolError, ToolResult};

/// The inputs one model request is assembled from. `Default` is an agent
/// with no identity files, no history, no memory, no skills and no MCP
/// servers, so a test sets only what it varies.
#[derive(Default)]
pub struct RequestInputs {
    pub identity: IdentityFiles,
    pub recent_messages: RecentMessages,
    /// The observation log, as the memory context carries it.
    pub observations: Option<String>,
    /// The narrative summary from the most recent observation.
    pub recent_context: Option<String>,
    /// The skills index XML.
    pub skills_index: Option<String>,
    /// The active skills' instructions XML.
    pub active_skills: Option<String>,
    /// The MCP registry whose tools join the request; `None` means no MCP
    /// servers are connected.
    pub mcp: Option<SharedMcpRegistry>,
    /// The turn's options, cache hints included.
    pub options: CompletionOptions,
}

/// Render the request a turn would send for `inputs` and `tools`, through the
/// same builder the turn loop calls, with no provider call.
///
/// `tools` is the agent's tool registry. A caller in a crate that depends on
/// `residuum-tools` builds it with the built-in set (`register_defaults`).
/// This crate cannot, since `residuum-tools` depends on it.
#[must_use]
pub async fn render_model_request(inputs: &RequestInputs, tools: &ToolRegistry) -> ModelRequest {
    let mcp = inputs.mcp.clone().unwrap_or_else(McpRegistry::new_shared);
    let memory_ctx = MemoryContext {
        observations: inputs.observations.as_deref(),
        recent_context: inputs.recent_context.as_deref(),
    };
    let prompt_ctx = PromptContext {
        skills: SkillsContext {
            index: inputs.skills_index.as_deref(),
            active_instructions: inputs.active_skills.as_deref(),
        },
        ..PromptContext::default()
    };
    build_model_request(
        &inputs.identity,
        &inputs.recent_messages,
        &memory_ctx,
        &prompt_ctx,
        tools,
        &mcp,
        &inputs.options,
    )
    .await
}

fn definition_named(name: &str) -> ToolDefinition {
    ToolDefinition {
        name: name.to_string(),
        description: String::new(),
        parameters: serde_json::json!({}),
    }
}

/// Answers every call with `"echoed"`: some tool for a turn to call.
pub struct EchoTool;

#[async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &'static str {
        "echo"
    }

    fn definition(&self) -> ToolDefinition {
        definition_named(self.name())
    }

    async fn execute(&self, _arguments: serde_json::Value) -> Result<ToolResult, ToolError> {
        Ok(ToolResult::success("echoed"))
    }
}

/// Counts how many times it actually ran and answers `"ok"`, so a test can
/// tell a call that ran from one that only produced a result some other way.
pub struct CountingTool {
    calls: Arc<AtomicUsize>,
}

impl CountingTool {
    /// A counting tool that adds each run to `calls`.
    #[must_use]
    pub fn new(calls: Arc<AtomicUsize>) -> Self {
        Self { calls }
    }
}

#[async_trait]
impl Tool for CountingTool {
    fn name(&self) -> &'static str {
        "counting_tool"
    }

    fn definition(&self) -> ToolDefinition {
        definition_named(self.name())
    }

    async fn execute(&self, _arguments: serde_json::Value) -> Result<ToolResult, ToolError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult::success("ok"))
    }
}

/// Holds at its gate, then never finishes, so a test can stop the turn while
/// the tool runs: the default `execute_cancellable` can only return through
/// its cancellation branch, which proves dispatch races against the stop
/// token instead of waiting for the tool.
pub struct BlockingTool {
    entry: GateEntry,
}

impl BlockingTool {
    /// A blocking tool held at `gate`.
    #[must_use]
    pub fn new(gate: &Gate) -> Self {
        Self {
            entry: gate.entry(),
        }
    }
}

#[async_trait]
impl Tool for BlockingTool {
    fn name(&self) -> &'static str {
        "blocking_tool"
    }

    fn definition(&self) -> ToolDefinition {
        definition_named(self.name())
    }

    async fn execute(&self, _arguments: serde_json::Value) -> Result<ToolResult, ToolError> {
        self.entry.pass().await;
        std::future::pending().await
    }
}

/// Answers every call with the output it was given, so a test controls
/// exactly what the turn loop receives from a tool.
pub struct FixedOutputTool {
    output: String,
}

impl FixedOutputTool {
    /// A tool that answers every call with `output`.
    #[must_use]
    pub fn new(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
        }
    }
}

#[async_trait]
impl Tool for FixedOutputTool {
    fn name(&self) -> &'static str {
        "fixed_output"
    }

    fn definition(&self) -> ToolDefinition {
        definition_named(self.name())
    }

    async fn execute(&self, _arguments: serde_json::Value) -> Result<ToolResult, ToolError> {
        Ok(ToolResult::success(self.output.clone()))
    }
}
