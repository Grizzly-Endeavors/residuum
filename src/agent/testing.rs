//! Test tools for driving the turn loop without the real tool set.
//!
//! Each tool has a fixed name and an empty parameter schema; a test's
//! scripted model calls it by that name.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use residuum_core::testing::gate::{Gate, GateEntry};

use super::Tool;
use crate::inference::{ToolDefinition, ToolError, ToolResult};

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
