//! Turn execution: the tool loop that drives the agent.

use tokio::sync::mpsc;

use crate::bus::{
    EndpointName, Publisher, ToolActivityEvent, ToolCallEvent, ToolResultEvent, topics,
};
use crate::mcp::SharedMcpRegistry;
use crate::models::{CompletionOptions, Message, ModelProvider, ModelResponse, ToolCall};
use crate::tools::{SharedToolFilter, ToolError, ToolFilter, ToolRegistry};
use crate::workspace::identity::IdentityFiles;
use anyhow::Context;

use super::context::{MemoryContext, PromptContext, StatusLine, assemble_system_prompt};
use super::interrupt::Interrupt;
use super::recent_messages::RecentMessages;

/// Maximum number of tool-call iterations before the agent stops.
pub(crate) const MAX_TOOL_ITERATIONS: usize = 50;

/// Context for publishing streaming events during a turn.
pub(crate) struct EventContext<'a> {
    pub publisher: &'a Publisher,
    pub output_endpoint: Option<&'a EndpointName>,
    pub tool_activity_endpoint: Option<&'a EndpointName>,
    pub correlation_id: &'a str,
}

/// Maximum retries for empty responses (transient API glitches).
const MAX_EMPTY_RESPONSE_RETRIES: u32 = 2;

/// Shared subsystem references needed for each turn iteration.
pub(crate) struct TurnResources<'a> {
    pub provider: &'a dyn ModelProvider,
    pub tools: &'a ToolRegistry,
    pub tool_filter: &'a SharedToolFilter,
    pub mcp_registry: &'a SharedMcpRegistry,
    pub identity: &'a IdentityFiles,
    pub options: &'a CompletionOptions,
}

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
pub(crate) async fn execute_turn(
    resources: &TurnResources<'_>,
    memory_ctx: &MemoryContext<'_>,
    prompt_ctx: &PromptContext<'_>,
    recent_messages: &mut RecentMessages,
    events: &EventContext<'_>,
    status_line: Option<&StatusLine>,
    interrupt_rx: &mut mpsc::Receiver<Interrupt>,
) -> anyhow::Result<Vec<String>> {
    let mut texts: Vec<String> = Vec::new();
    let mut empty_retries: u32 = 0;

    for iteration in 0..MAX_TOOL_ITERATIONS {
        drain_interrupts(interrupt_rx, recent_messages);

        // Clone the filter each iteration so the guard is dropped before tool
        // execution. Tools like project_activate need a write lock on the same
        // RwLock, which would deadlock if we held a read guard across the call.
        let filter = resources.tool_filter.read().await.clone();
        let mut tool_definitions = resources.tools.definitions(&filter);

        // Merge MCP tool definitions from all connected servers
        let mcp_guard = resources.mcp_registry.read().await;
        tool_definitions.extend(mcp_guard.tool_definitions());
        drop(mcp_guard);

        // System prompt is reassembled each iteration because tool execution
        // can modify identity files (e.g. write_file updating MEMORY.md).
        let messages = assemble_system_prompt(
            resources.identity,
            recent_messages,
            memory_ctx,
            prompt_ctx,
            status_line,
        );

        let mut response = resources
            .provider
            .complete(&messages, &tool_definitions, resources.options)
            .await
            .context("model completion failed")?;

        if let Some(ref thinking) = response.thinking {
            tracing::debug!(
                thinking_len = thinking.len(),
                "structured thinking received"
            );
        }
        response.content = crate::models::think_tags::strip_think_tags(&response.content);

        if response.tool_calls.is_empty() {
            log_usage(&response);
            if response.content.is_empty() {
                if empty_retries < MAX_EMPTY_RESPONSE_RETRIES {
                    empty_retries += 1;
                    tracing::warn!(
                        attempt = empty_retries,
                        max = MAX_EMPTY_RESPONSE_RETRIES,
                        "model returned empty response, retrying"
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    continue;
                }
                anyhow::bail!("model returned empty response with no tool calls");
            }
            tracing::debug!(iterations = iteration, "turn complete");
            recent_messages.push(Message::assistant(response.content.clone(), None));
            texts.push(response.content);
            return Ok(texts);
        }

        tracing::info!(
            iteration,
            tool_count = response.tool_calls.len(),
            "processing tool calls"
        );

        if !response.content.is_empty()
            && let Some(ep) = events.output_endpoint
            && let Err(e) = events
                .publisher
                .publish(
                    topics::Endpoint(ep.clone()),
                    crate::bus::IntermediateEvent {
                        correlation_id: events.correlation_id.to_owned(),
                        content: response.content.clone(),
                    },
                )
                .await
        {
            tracing::debug!(error = %e, "failed to publish intermediate text event");
        }

        recent_messages.push(Message::assistant(
            response.content.clone(),
            Some(response.tool_calls.clone()),
        ));

        for tool_call in &response.tool_calls {
            execute_tool(tool_call, resources, &filter, recent_messages, events).await;
        }

        log_usage(&response);
    }

    anyhow::bail!("agent exceeded maximum tool iterations ({MAX_TOOL_ITERATIONS})")
}

/// Drain any interrupt messages that arrived while tools were executing.
fn drain_interrupts(
    interrupt_rx: &mut mpsc::Receiver<Interrupt>,
    recent_messages: &mut RecentMessages,
) {
    while let Ok(interrupt) = interrupt_rx.try_recv() {
        match interrupt {
            Interrupt::UserMessage(msg) => {
                tracing::info!(msg_id = %msg.id, "injecting mid-turn user message");
                recent_messages.push(Message::user(msg.content));
            }
            Interrupt::BackgroundResult(result) => {
                tracing::info!(
                    task_id = %result.task_id,
                    source = %result.source_label,
                    "injecting background result mid-turn"
                );
                recent_messages.push(Message::user(result.format_for_agent()));
            }
        }
    }
}

/// Execute a single tool call, falling back to MCP servers.
async fn execute_tool(
    tool_call: &ToolCall,
    resources: &TurnResources<'_>,
    filter: &ToolFilter,
    recent_messages: &mut RecentMessages,
    events: &EventContext<'_>,
) {
    if let Some(ep) = events.tool_activity_endpoint
        && let Err(e) = events
            .publisher
            .publish(
                topics::Endpoint(ep.clone()),
                ToolActivityEvent::Call(ToolCallEvent {
                    correlation_id: events.correlation_id.to_owned(),
                    tool_call_id: tool_call.id.clone(),
                    name: tool_call.name.clone(),
                    arguments: tool_call.arguments.clone(),
                }),
            )
            .await
    {
        tracing::debug!(error = %e, tool_name = %tool_call.name, "failed to publish tool call event");
    }

    // Try built-in tools first, fall back to MCP servers
    let mut source = "built-in";
    let result = match resources
        .tools
        .execute(&tool_call.name, tool_call.arguments.clone(), filter)
        .await
    {
        Err(ToolError::NotFound(_)) => {
            tracing::debug!(tool_name = %tool_call.name, "tool not found in built-in registry, falling back to MCP");
            source = "mcp";
            resources
                .mcp_registry
                .read()
                .await
                .call_tool(&tool_call.name, tool_call.arguments.clone())
                .await
        }
        other => other,
    };

    let (output, is_error, images) = match result {
        Ok(r) => (r.output, r.is_error, r.images),
        Err(e) => {
            tracing::warn!(
                error = %e,
                tool_name = %tool_call.name,
                tool_call_id = %tool_call.id,
                source,
                "tool execution failed"
            );
            (e.to_string(), true, vec![])
        }
    };

    if let Some(ep) = events.tool_activity_endpoint
        && let Err(e) = events
            .publisher
            .publish(
                topics::Endpoint(ep.clone()),
                ToolActivityEvent::Result(ToolResultEvent {
                    correlation_id: events.correlation_id.to_owned(),
                    tool_call_id: tool_call.id.clone(),
                    name: tool_call.name.clone(),
                    output: output.clone(),
                    is_error,
                }),
            )
            .await
    {
        tracing::debug!(error = %e, tool_name = %tool_call.name, "failed to publish tool result event");
    }

    if images.is_empty() {
        recent_messages.push(Message::tool(output, tool_call.id.clone()));
    } else {
        recent_messages.push(Message::tool_with_images(
            output,
            tool_call.id.clone(),
            images,
        ));
    }
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
    } else {
        tracing::debug!("token usage not available in response");
    }
}
