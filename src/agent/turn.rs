//! Turn execution: the tool loop that drives the agent.

use tokio::sync::mpsc;

use crate::error::ResiduumError;
use crate::interfaces::types::ReplyHandle;
use crate::mcp::SharedMcpRegistry;
use crate::models::{CompletionOptions, Message, ModelProvider, ModelResponse};
use crate::tools::{SharedToolFilter, ToolError, ToolRegistry};
use crate::workspace::identity::IdentityFiles;

use super::context::{MemoryContext, PromptContext, StatusLine, assemble_system_prompt};
use super::interrupt::Interrupt;
use super::recent_messages::RecentMessages;

/// Maximum number of tool-call iterations before the agent stops.
pub(crate) const MAX_TOOL_ITERATIONS: usize = 50;

/// Execute the tool loop against the given message buffer.
///
/// Calls the provider repeatedly until it returns a text response (no tool calls),
/// executing any requested tools in between. Updates `recent_messages` in place.
///
/// MCP tool definitions are merged into the built-in tool list, and tool calls
/// fall back to MCP servers when no built-in tool matches.
///
/// Returns a vec containing the final text-only response. Intermediate texts
/// emitted alongside tool calls are sent via `reply` in real-time but not
/// included in the return value.
#[expect(
    clippy::too_many_arguments,
    reason = "threading context through the turn loop; grouping into a struct would obscure the call site"
)]
pub(crate) async fn execute_turn(
    provider: &dyn ModelProvider,
    tools: &ToolRegistry,
    tool_filter: &SharedToolFilter,
    mcp_registry: &SharedMcpRegistry,
    identity: &IdentityFiles,
    options: &CompletionOptions,
    memory_ctx: &MemoryContext<'_>,
    prompt_ctx: &PromptContext<'_>,
    recent_messages: &mut RecentMessages,
    reply: &dyn ReplyHandle,
    status_line: Option<&StatusLine>,
    interrupt_rx: &mut mpsc::Receiver<Interrupt>,
) -> Result<Vec<String>, ResiduumError> {
    let mut texts: Vec<String> = Vec::new();

    for iteration in 0..MAX_TOOL_ITERATIONS {
        // === Interrupt check point ===
        // Drain any messages that arrived while tools were executing.
        while let Ok(interrupt) = interrupt_rx.try_recv() {
            match interrupt {
                Interrupt::UserMessage(msg) => {
                    tracing::info!(msg_id = %msg.id, "injecting mid-turn user message");
                    recent_messages.push(Message::user(msg.content));
                }
                Interrupt::BackgroundResult(result) => {
                    tracing::info!(
                        task_id = %result.id,
                        task_name = %result.task_name,
                        "injecting background task result"
                    );
                    recent_messages.push(Message::system(
                        crate::background::types::format_background_result(&result),
                    ));
                }
            }
        }
        // Clone the filter each iteration so the guard is dropped before tool
        // execution. Tools like project_activate need a write lock on the same
        // RwLock, which would deadlock if we held a read guard across the call.
        let filter = tool_filter.read().await.clone();
        let mut tool_definitions = tools.definitions(&filter);

        // Merge MCP tool definitions from all connected servers
        let mcp_guard = mcp_registry.read().await;
        tool_definitions.extend(mcp_guard.tool_definitions());
        drop(mcp_guard);

        // System prompt is reassembled each iteration because tool execution
        // can modify identity files (e.g. write_file updating MEMORY.md).
        let messages = assemble_system_prompt(
            identity,
            recent_messages,
            memory_ctx,
            prompt_ctx,
            status_line,
        );

        let response = provider
            .complete(&messages, &tool_definitions, options)
            .await
            .map_err(ResiduumError::Model)?;

        // Strip <think>...</think> blocks emitted by reasoning models (e.g. DeepSeek-R1)
        let mut response = response;
        response.content = crate::models::think_tags::strip_think_tags(&response.content);

        if response.tool_calls.is_empty() {
            recent_messages.push(Message::assistant(response.content.clone(), None));
            log_usage(&response);

            if response.content.is_empty() {
                tracing::warn!("model returned empty response with no tool calls");
                return Err(ResiduumError::Other(anyhow::anyhow!(
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

        // Send any text the model emitted alongside tool calls in real-time.
        if !response.content.is_empty() {
            reply.send_intermediate(&response.content).await;
        }

        recent_messages.push(Message::assistant(
            response.content.clone(),
            Some(response.tool_calls.clone()),
        ));

        // TODO(phase-4): add security boundary before Discord integration
        for tool_call in &response.tool_calls {
            reply
                .send_tool_call(&tool_call.id, &tool_call.name, &tool_call.arguments)
                .await;

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

            reply
                .send_tool_result(&tool_call.id, &tool_call.name, &output, is_error)
                .await;

            recent_messages.push(Message::tool(output, tool_call.id.clone()));
        }

        log_usage(&response);
    }

    Err(ResiduumError::Other(anyhow::anyhow!(
        "agent exceeded maximum tool iterations ({MAX_TOOL_ITERATIONS})"
    )))
}

/// Log token usage from a model response at info level.
fn log_usage(response: &ModelResponse) {
    if let Some(usage) = response.usage {
        tracing::info!(
            input_tokens = usage.input_tokens,
            output_tokens = usage.output_tokens,
            cache_creation_tokens = usage.cache_creation_tokens,
            cache_read_tokens = usage.cache_read_tokens,
            "token usage"
        );
    }
}
