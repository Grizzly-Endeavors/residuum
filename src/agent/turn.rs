//! Turn execution: the tool loop that drives the agent.

use crate::channels::TurnDisplay;
use crate::error::IronclawError;
use crate::mcp::SharedMcpRegistry;
use crate::models::{CompletionOptions, Message, ModelProvider, ModelResponse};
use crate::tools::{SharedToolFilter, ToolError, ToolRegistry};
use crate::workspace::identity::IdentityFiles;

use super::context::{
    MemoryContext, ProjectsContext, SkillsContext, TimeContext, assemble_system_prompt,
};
use super::recent_messages::RecentMessages;

/// Maximum number of tool-call iterations before the agent stops.
pub(super) const MAX_TOOL_ITERATIONS: usize = 50;

/// Execute the tool loop against the given message buffer.
///
/// Calls the provider repeatedly until it returns a text response (no tool calls),
/// executing any requested tools in between. Updates `recent_messages` in place.
///
/// MCP tool definitions are merged into the built-in tool list, and tool calls
/// fall back to MCP servers when no built-in tool matches.
///
/// Returns a vec containing the final text-only response. Intermediate texts
/// emitted alongside tool calls are broadcast via `display` in real-time but
/// not included in the return value.
#[expect(
    clippy::too_many_arguments,
    reason = "threading context through the turn loop; grouping into a struct would obscure the call site"
)]
pub(super) async fn execute_turn(
    provider: &dyn ModelProvider,
    tools: &ToolRegistry,
    tool_filter: &SharedToolFilter,
    mcp_registry: &SharedMcpRegistry,
    identity: &IdentityFiles,
    options: &CompletionOptions,
    memory_ctx: &MemoryContext<'_>,
    projects_ctx: &ProjectsContext<'_>,
    skills_ctx: &SkillsContext<'_>,
    recent_messages: &mut RecentMessages,
    display: &dyn TurnDisplay,
    time_ctx: Option<&TimeContext>,
) -> Result<Vec<String>, IronclawError> {
    let filter = tool_filter.read().await;
    let mut tool_definitions = tools.definitions(&filter);

    // Merge MCP tool definitions from all connected servers
    let mcp_guard = mcp_registry.read().await;
    tool_definitions.extend(mcp_guard.tool_definitions());
    drop(mcp_guard);

    let mut texts: Vec<String> = Vec::new();

    for iteration in 0..MAX_TOOL_ITERATIONS {
        // System prompt is reassembled each iteration because tool execution
        // can modify identity files (e.g. write_file updating MEMORY.md).
        let messages = assemble_system_prompt(
            identity,
            recent_messages,
            memory_ctx,
            projects_ctx,
            skills_ctx,
            time_ctx,
        );

        let response = provider
            .complete(&messages, &tool_definitions, options)
            .await
            .map_err(IronclawError::Model)?;

        if response.tool_calls.is_empty() {
            recent_messages.push(Message::assistant(response.content.clone(), None));
            log_usage(&response);

            if response.content.is_empty() {
                tracing::warn!("model returned empty response with no tool calls");
                return Err(IronclawError::Other(anyhow::anyhow!(
                    "model returned empty response with no tool calls"
                )));
            }

            texts.push(response.content);
            return Ok(texts);
        }

        tracing::info!(
            iteration,
            tool_count = response.tool_calls.len(),
            "processing tool calls"
        );

        // Broadcast any text the model emitted alongside tool calls in real-time.
        if !response.content.is_empty() {
            display.show_response(&response.content);
        }

        recent_messages.push(Message::assistant(
            response.content.clone(),
            Some(response.tool_calls.clone()),
        ));

        // TODO(phase-4): add security boundary before Discord integration
        for tool_call in &response.tool_calls {
            display.show_tool_call(&tool_call.name, &tool_call.arguments);

            // Try built-in tools first, fall back to MCP servers
            let result = match tools
                .execute(&tool_call.name, tool_call.arguments.clone(), &filter)
                .await
            {
                Err(ToolError::NotFound(_)) => {
                    mcp_registry
                        .read()
                        .await
                        .call_tool(&tool_call.name, tool_call.arguments.clone())
                        .await
                }
                other => other,
            };

            let (output, is_error) = match result {
                Ok(r) => (r.output, r.is_error),
                Err(e) => (e.to_string(), true),
            };

            display.show_tool_result(&tool_call.name, &output, is_error);

            recent_messages.push(Message::tool(output, tool_call.id.clone()));
        }

        log_usage(&response);
    }

    Err(IronclawError::Other(anyhow::anyhow!(
        "agent exceeded maximum tool iterations ({MAX_TOOL_ITERATIONS})"
    )))
}

/// Log token usage from a model response at info level.
fn log_usage(response: &ModelResponse) {
    if let Some(usage) = response.usage {
        tracing::info!(
            input_tokens = usage.input_tokens,
            output_tokens = usage.output_tokens,
            "token usage"
        );
    }
}
