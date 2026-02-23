//! Agent runtime: context assembly, tool loop, and message history management.

pub mod context;
pub mod recent_messages;

use crate::channels::TurnDisplay;
use crate::channels::types::MessageOrigin;
use crate::error::IronclawError;
use crate::models::{CompletionOptions, Message, ModelProvider, ModelResponse};
use crate::tools::ToolRegistry;
use crate::workspace::identity::IdentityFiles;

use self::context::{MemoryContext, TimeContext, assemble_system_prompt};
use self::recent_messages::RecentMessages;

/// Maximum number of tool-call iterations before the agent stops.
const MAX_TOOL_ITERATIONS: usize = 50;

/// Result of a background system turn (pulse or cron).
pub struct SystemTurnResult {
    /// The assistant's final text response.
    pub response: String,
    /// All messages from the background thread (user prompt + assistant response + tool calls).
    ///
    /// Feed these into `run_memory_pipeline()` so background turns contribute to memory.
    pub messages: Vec<Message>,
}

/// The agent runtime that processes user messages through the model.
pub struct Agent {
    provider: Box<dyn ModelProvider>,
    tools: ToolRegistry,
    identity: IdentityFiles,
    recent_messages: RecentMessages,
    options: CompletionOptions,
    observations: Option<String>,
    /// Narrative summary from the most recent observation cycle.
    recent_context: Option<String>,
    /// System event texts queued from cron jobs; injected at the start of the next user turn.
    pending_system_events: Vec<String>,
    tz: chrono_tz::Tz,
    last_user_message_at: Option<chrono::NaiveDateTime>,
}

impl Agent {
    /// Create a new agent with the given components.
    #[must_use]
    pub fn new(
        provider: Box<dyn ModelProvider>,
        tools: ToolRegistry,
        identity: IdentityFiles,
        options: CompletionOptions,
        tz: chrono_tz::Tz,
    ) -> Self {
        Self {
            provider,
            tools,
            identity,
            recent_messages: RecentMessages::new(),
            options,
            observations: None,
            recent_context: None,
            pending_system_events: Vec::new(),
            tz,
            last_user_message_at: None,
        }
    }

    /// Reload observations from the observation log file.
    ///
    /// Deserializes the JSON into an `ObservationLog` and formats it as
    /// human-readable text for the system prompt.
    ///
    /// # Errors
    /// Returns an error if the file exists but cannot be read or parsed.
    pub async fn reload_observations(
        &mut self,
        layout: &crate::workspace::layout::WorkspaceLayout,
    ) -> Result<(), IronclawError> {
        let path = layout.observations_json();
        match tokio::fs::read_to_string(&path).await {
            Ok(content) if !content.trim().is_empty() => {
                let log: crate::memory::types::ObservationLog = serde_json::from_str(&content)
                    .map_err(|e| {
                        IronclawError::Memory(format!(
                            "failed to parse observations at {}: {e}",
                            path.display()
                        ))
                    })?;
                let formatted = log.display_formatted();
                self.observations = if formatted.is_empty() {
                    None
                } else {
                    Some(formatted)
                };
            }
            Ok(_) => {
                self.observations = None;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.observations = None;
            }
            Err(e) => {
                return Err(IronclawError::Memory(format!(
                    "failed to read observations at {}: {e}",
                    path.display()
                )));
            }
        }
        Ok(())
    }

    /// Reload narrative context from the `recent_context.json` file.
    ///
    /// # Errors
    /// Returns an error if the file exists but cannot be parsed.
    pub async fn reload_recent_context(
        &mut self,
        layout: &crate::workspace::layout::WorkspaceLayout,
    ) -> Result<(), IronclawError> {
        let path = layout.recent_context_json();
        match crate::memory::recent_store::load_recent_context(&path).await {
            Ok(Some(ctx)) => {
                self.recent_context = Some(ctx.narrative);
            }
            Ok(None) => {
                self.recent_context = None;
            }
            Err(e) => return Err(e),
        }
        Ok(())
    }

    /// Restore persisted messages into the recent history.
    ///
    /// Used at startup to reload unobserved messages from `recent_messages.json`
    /// so the agent retains context from the previous run.
    pub fn restore_messages(&mut self, messages: Vec<Message>) {
        for msg in messages {
            self.recent_messages.push(msg);
        }
    }

    /// Seed the last user message timestamp from persisted data.
    ///
    /// Called at startup so the first time context tag after a restart
    /// shows the correct "last message" duration.
    pub fn set_last_user_message_at(&mut self, at: Option<chrono::NaiveDateTime>) {
        self.last_user_message_at = at;
    }

    /// Clear all messages from the recent history.
    ///
    /// Called after the observer fires so observed messages don't linger
    /// in both the recent messages and the observation log.
    pub fn clear_recent_messages(&mut self) {
        self.recent_messages.clear();
    }

    /// Rotate messages after an observation cycle.
    ///
    /// Extracts the last 3 text exchanges, clears the buffer, then prepends
    /// the retained exchanges so the agent keeps conversational context.
    pub fn rotate_messages_after_observation(&mut self) {
        let retained = self.recent_messages.last_exchanges(3);
        self.recent_messages.clear();
        self.recent_messages.prepend(retained);
    }

    /// Queue a system event to be injected at the start of the next user turn.
    ///
    /// Used by the cron executor for `Delivery::UserVisible` jobs.
    pub fn queue_system_event(&mut self, text: String) {
        self.pending_system_events.push(text);
    }

    /// Process a user message through the model, executing tool calls as needed.
    ///
    /// Any queued system events are prepended to the user message so the agent
    /// sees pending alerts in context. Returns a vec containing the final
    /// text-only response. Intermediate texts emitted alongside tool calls are
    /// broadcast via `display` in real-time but not included in the return value.
    ///
    /// # Errors
    /// Returns `IronclawError` if the model call fails or tool execution errors
    /// are unrecoverable.
    pub async fn process_message(
        &mut self,
        user_input: &str,
        display: &dyn TurnDisplay,
        origin: Option<&MessageOrigin>,
    ) -> Result<Vec<String>, IronclawError> {
        let now = crate::time::now_local(self.tz);
        let time_ctx = TimeContext {
            now,
            last_message_at: self.last_user_message_at,
            message_source: origin.map(|o| o.channel.clone()),
        };
        self.last_user_message_at = Some(now);

        // Build effective input: prepend any pending system events
        let effective_input = if self.pending_system_events.is_empty() {
            user_input.to_string()
        } else {
            let events: Vec<String> = self.pending_system_events.drain(..).collect();
            let events_text = events.join("\n\n");
            format!("[System Alerts — please review]\n\n{events_text}\n\n---\n\n{user_input}")
        };

        self.recent_messages.push(Message::user(effective_input));

        let memory_ctx = MemoryContext {
            observations: self.observations.as_deref(),
            recent_context: self.recent_context.as_deref(),
        };

        execute_turn(
            &*self.provider,
            &self.tools,
            &self.identity,
            &self.options,
            &memory_ctx,
            &mut self.recent_messages,
            display,
            Some(&time_ctx),
        )
        .await
    }

    /// Run a background agent thread for pulse or cron tasks.
    ///
    /// Creates a temporary message buffer that is not added to the main conversation.
    /// Returns both the response text and the thread messages so the caller
    /// can feed them into `run_memory_pipeline()`.
    ///
    /// If `provider_override` is `Some`, that provider is used instead of the
    /// agent's default provider for this turn only.
    ///
    /// # Errors
    /// Returns `IronclawError` if the model call fails.
    pub async fn run_system_turn(
        &self,
        prompt: &str,
        display: &dyn TurnDisplay,
        provider_override: Option<&dyn ModelProvider>,
    ) -> Result<SystemTurnResult, IronclawError> {
        let mut thread_messages = RecentMessages::new();
        thread_messages.push(Message::user(prompt));

        let provider: &dyn ModelProvider = provider_override.unwrap_or(&*self.provider);

        let memory_ctx = MemoryContext {
            observations: self.observations.as_deref(),
            recent_context: self.recent_context.as_deref(),
        };

        // System turns don't inject time context (no user-facing timestamps)
        let texts = execute_turn(
            provider,
            &self.tools,
            &self.identity,
            &self.options,
            &memory_ctx,
            &mut thread_messages,
            display,
            None,
        )
        .await?;

        let response = texts.last().cloned().unwrap_or_default();

        Ok(SystemTurnResult {
            response,
            messages: thread_messages.messages().to_vec(),
        })
    }

    /// Get the current recent message count.
    #[must_use]
    pub fn message_count(&self) -> usize {
        self.recent_messages.len()
    }

    /// Get messages added since the given index.
    #[must_use]
    pub fn messages_since(&self, idx: usize) -> &[Message] {
        self.recent_messages.messages_since(idx)
    }
}

/// Execute the tool loop against the given message buffer.
///
/// Calls the provider repeatedly until it returns a text response (no tool calls),
/// executing any requested tools in between. Updates `recent_messages` in place.
///
/// Returns a vec containing the final text-only response. Intermediate texts
/// emitted alongside tool calls are broadcast via `display` in real-time but
/// not included in the return value.
#[expect(
    clippy::too_many_arguments,
    reason = "adding time_ctx pushes past 7; grouping into a struct would obscure the call site"
)]
async fn execute_turn(
    provider: &dyn ModelProvider,
    tools: &ToolRegistry,
    identity: &IdentityFiles,
    options: &CompletionOptions,
    memory_ctx: &MemoryContext<'_>,
    recent_messages: &mut RecentMessages,
    display: &dyn TurnDisplay,
    time_ctx: Option<&TimeContext>,
) -> Result<Vec<String>, IronclawError> {
    let tool_definitions = tools.definitions();
    let mut texts: Vec<String> = Vec::new();

    for iteration in 0..MAX_TOOL_ITERATIONS {
        // System prompt is reassembled each iteration because tool execution
        // can modify identity files (e.g. write_file updating MEMORY.md).
        let messages = assemble_system_prompt(identity, recent_messages, memory_ctx, time_ctx);

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

            let result = tools
                .execute(&tool_call.name, tool_call.arguments.clone())
                .await;

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

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "test code uses unwrap and indexing for clarity"
)]
mod tests {
    use super::*;
    use crate::channels::null::NullDisplay;
    use crate::models::{ModelError, ToolCall, ToolDefinition};
    use crate::tools::FileTracker;
    use async_trait::async_trait;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Mock provider that returns pre-configured responses in sequence.
    ///
    /// Intentionally duplicated across agent, observer, and reflector tests — each mock
    /// has slightly different fields. Extract a shared mock when a 4th instance appears.
    struct MockProvider {
        responses: Vec<ModelResponse>,
        call_count: Arc<AtomicUsize>,
    }

    impl MockProvider {
        fn new(responses: Vec<ModelResponse>) -> Self {
            Self {
                responses,
                call_count: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    #[async_trait]
    impl ModelProvider for MockProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _options: &CompletionOptions,
        ) -> Result<ModelResponse, ModelError> {
            let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
            self.responses
                .get(idx)
                .cloned()
                .ok_or_else(|| ModelError::Api("no more mock responses".to_string()))
        }

        fn model_name(&self) -> &'static str {
            "mock-model"
        }
    }

    #[tokio::test]
    async fn single_text_response() {
        let provider =
            MockProvider::new(vec![ModelResponse::new("hello there".to_string(), vec![])]);

        let mut agent = Agent::new(
            Box::new(provider),
            ToolRegistry::new(),
            IdentityFiles::default(),
            CompletionOptions::default(),
            chrono_tz::UTC,
        );

        let display = NullDisplay;
        let result = agent.process_message("hi", &display, None).await.unwrap();
        assert_eq!(result, vec!["hello there"], "should return model text");
    }

    #[tokio::test]
    async fn tool_loop_then_text() {
        let mut registry = ToolRegistry::new();
        registry.register_defaults(FileTracker::new_shared());

        let provider = MockProvider::new(vec![
            ModelResponse::new(
                String::new(),
                vec![ToolCall {
                    id: "call_1".to_string(),
                    name: "exec".to_string(),
                    arguments: serde_json::json!({"command": "echo test"}),
                }],
            ),
            ModelResponse::new("the result was: test".to_string(), vec![]),
        ]);

        let mut agent = Agent::new(
            Box::new(provider),
            registry,
            IdentityFiles::default(),
            CompletionOptions::default(),
            chrono_tz::UTC,
        );

        let display = NullDisplay;
        let result = agent
            .process_message("run echo test", &display, None)
            .await
            .unwrap();
        assert_eq!(
            result,
            vec!["the result was: test"],
            "should return final text after tool loop"
        );
    }

    #[tokio::test]
    async fn intermediate_text_broadcast_not_returned() {
        let mut registry = ToolRegistry::new();
        registry.register_defaults(FileTracker::new_shared());

        // First response has text alongside tool calls (intermediate), second is final.
        let provider = MockProvider::new(vec![
            ModelResponse::new(
                "Let me check that for you...".to_string(),
                vec![ToolCall {
                    id: "call_1".to_string(),
                    name: "exec".to_string(),
                    arguments: serde_json::json!({"command": "echo test"}),
                }],
            ),
            ModelResponse::new("Done! The output was: test".to_string(), vec![]),
        ]);

        let mut agent = Agent::new(
            Box::new(provider),
            registry,
            IdentityFiles::default(),
            CompletionOptions::default(),
            chrono_tz::UTC,
        );

        let display = NullDisplay;
        let result = agent
            .process_message("what does echo test print?", &display, None)
            .await
            .unwrap();
        assert_eq!(
            result,
            vec!["Done! The output was: test"],
            "should return only final text, intermediate is broadcast via display"
        );
    }

    #[tokio::test]
    async fn max_iterations_guard() {
        let responses: Vec<ModelResponse> = (0..=MAX_TOOL_ITERATIONS)
            .map(|i| {
                ModelResponse::new(
                    String::new(),
                    vec![ToolCall {
                        id: format!("call_{i}"),
                        name: "exec".to_string(),
                        arguments: serde_json::json!({"command": "echo loop"}),
                    }],
                )
            })
            .collect();

        let mut registry = ToolRegistry::new();
        registry.register_defaults(FileTracker::new_shared());

        let provider = MockProvider::new(responses);
        let mut agent = Agent::new(
            Box::new(provider),
            registry,
            IdentityFiles::default(),
            CompletionOptions::default(),
            chrono_tz::UTC,
        );

        let display = NullDisplay;
        let result = agent.process_message("loop forever", &display, None).await;
        assert!(result.is_err(), "should error after max iterations");
    }

    #[tokio::test]
    async fn run_system_turn_ephemeral() {
        let provider =
            MockProvider::new(vec![ModelResponse::new("HEARTBEAT_OK".to_string(), vec![])]);

        let agent = Agent::new(
            Box::new(provider),
            ToolRegistry::new(),
            IdentityFiles::default(),
            CompletionOptions::default(),
            chrono_tz::UTC,
        );

        let display = NullDisplay;
        let result = agent
            .run_system_turn("check status", &display, None)
            .await
            .unwrap();
        assert_eq!(result.response, "HEARTBEAT_OK", "response should match");
        assert!(
            !result.messages.is_empty(),
            "should have ephemeral messages"
        );
        assert_eq!(
            agent.message_count(),
            0,
            "main message history should be untouched"
        );
    }

    #[tokio::test]
    async fn pending_system_events_prepended() {
        let provider =
            MockProvider::new(vec![ModelResponse::new("acknowledged".to_string(), vec![])]);

        let mut agent = Agent::new(
            Box::new(provider),
            ToolRegistry::new(),
            IdentityFiles::default(),
            CompletionOptions::default(),
            chrono_tz::UTC,
        );

        agent.queue_system_event("email arrived from boss".to_string());
        let display = NullDisplay;
        agent
            .process_message("what's up?", &display, None)
            .await
            .unwrap();

        // Queue should be drained
        let msgs = agent.messages_since(0);
        assert_eq!(msgs.len(), 2, "should have user + assistant messages");
        assert!(
            msgs.first()
                .is_some_and(|m| m.content.contains("email arrived from boss")),
            "system event should be in user message"
        );
    }

    #[test]
    fn rotate_messages_retains_last_exchanges() {
        let mut agent = Agent::new(
            Box::new(MockProvider::new(vec![])),
            ToolRegistry::new(),
            IdentityFiles::default(),
            CompletionOptions::default(),
            chrono_tz::UTC,
        );

        // Simulate a conversation with 5 exchanges
        for i in 0..5 {
            agent
                .recent_messages
                .push(Message::user(format!("question {i}")));
            agent
                .recent_messages
                .push(Message::assistant(format!("answer {i}"), None));
        }
        assert_eq!(agent.message_count(), 10, "should have 10 messages");

        agent.rotate_messages_after_observation();

        // Should retain last 3 exchanges = 6 messages
        assert_eq!(
            agent.message_count(),
            6,
            "should retain 6 messages (3 exchanges)"
        );

        let msgs = agent.messages_since(0);
        assert_eq!(
            msgs[0].content, "question 2",
            "first retained should be exchange 2"
        );
        assert_eq!(msgs[1].content, "answer 2");
        assert_eq!(msgs[4].content, "question 4");
        assert_eq!(
            msgs[5].content, "answer 4",
            "last retained should be exchange 4"
        );
    }

    #[tokio::test]
    async fn empty_response_returns_error() {
        let provider = MockProvider::new(vec![ModelResponse::new(String::new(), vec![])]);

        let mut agent = Agent::new(
            Box::new(provider),
            ToolRegistry::new(),
            IdentityFiles::default(),
            CompletionOptions::default(),
            chrono_tz::UTC,
        );

        let display = NullDisplay;
        let result = agent.process_message("hello", &display, None).await;
        assert!(result.is_err(), "empty response should return error");

        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("empty response"),
            "error should mention empty response, got: {err_msg}"
        );
    }
}
