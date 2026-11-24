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

#[derive(Default)]
pub struct RequestInputs {
    pub identity: IdentityFiles,
    pub recent_messages: RecentMessages,
    pub observations: Option<String>,
    pub recent_context: Option<String>,
    pub skills_index: Option<String>,
    pub active_skills: Option<String>,
    pub mcp: Option<SharedMcpRegistry>,
    pub options: CompletionOptions,
}

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

pub struct CountingTool {
    calls: Arc<AtomicUsize>,
}

impl CountingTool {
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

pub struct BlockingTool {
    entry: GateEntry,
}

impl BlockingTool {
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

pub struct FixedOutputTool {
    output: String,
}

impl FixedOutputTool {
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
