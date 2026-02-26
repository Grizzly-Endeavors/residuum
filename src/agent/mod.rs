//! Agent runtime: context assembly, tool loop, and message history management.

pub mod context;
pub mod interrupt;
pub mod recent_messages;
pub(crate) mod turn;

use crate::channels::TurnDisplay;
use crate::channels::types::MessageOrigin;
use crate::error::IronclawError;
use crate::mcp::SharedMcpRegistry;
use crate::models::{CompletionOptions, Message, ModelProvider};
use crate::tools::{SharedToolFilter, ToolRegistry};
use crate::workspace::identity::IdentityFiles;

use self::context::{MemoryContext, PromptContext, StatusLine};
use self::recent_messages::RecentMessages;
use self::turn::execute_turn;

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
    tool_filter: SharedToolFilter,
    mcp_registry: SharedMcpRegistry,
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
    /// Path to the inbox directory (for computing unread count per turn).
    inbox_dir: std::path::PathBuf,
}

impl Agent {
    /// Create a new agent with the given components.
    #[must_use]
    #[expect(
        clippy::too_many_arguments,
        reason = "agent needs all subsystem handles at construction"
    )]
    pub fn new(
        provider: Box<dyn ModelProvider>,
        tools: ToolRegistry,
        tool_filter: SharedToolFilter,
        mcp_registry: SharedMcpRegistry,
        identity: IdentityFiles,
        options: CompletionOptions,
        tz: chrono_tz::Tz,
        inbox_dir: std::path::PathBuf,
    ) -> Self {
        Self {
            provider,
            tools,
            tool_filter,
            mcp_registry,
            identity,
            recent_messages: RecentMessages::new(),
            options,
            observations: None,
            recent_context: None,
            pending_system_events: Vec::new(),
            tz,
            last_user_message_at: None,
            inbox_dir,
        }
    }

    /// Get a reference to the MCP registry.
    #[must_use]
    pub fn mcp_registry(&self) -> &SharedMcpRegistry {
        &self.mcp_registry
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
        self.recent_context = crate::memory::recent_context::load_recent_context(&path)
            .await?
            .map(|ctx| ctx.narrative);
        Ok(())
    }

    /// Restore persisted messages into the recent history.
    ///
    /// Used at startup to reload unobserved messages from `recent_messages.json`
    /// so the agent retains context from the previous run.
    pub fn restore_messages(&mut self, messages: Vec<Message>) {
        self.recent_messages.extend(messages);
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
        prompt_ctx: &PromptContext<'_>,
        interrupt_rx: &mut tokio::sync::mpsc::Receiver<interrupt::Interrupt>,
    ) -> Result<Vec<String>, IronclawError> {
        let now = crate::time::now_local(self.tz);
        let unread = crate::inbox::count_unread(&self.inbox_dir);
        let status_line = StatusLine {
            now,
            last_message_at: self.last_user_message_at,
            message_source: origin.map(|o| o.channel.clone()),
            unread_inbox_count: unread,
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
            &self.tool_filter,
            &self.mcp_registry,
            &self.identity,
            &self.options,
            &memory_ctx,
            prompt_ctx,
            &mut self.recent_messages,
            display,
            Some(&status_line),
            interrupt_rx,
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
        prompt_ctx: &PromptContext<'_>,
    ) -> Result<SystemTurnResult, IronclawError> {
        let mut thread_messages = RecentMessages::new();
        thread_messages.push(Message::user(prompt));

        let provider: &dyn ModelProvider = provider_override.unwrap_or(&*self.provider);

        let memory_ctx = MemoryContext {
            observations: self.observations.as_deref(),
            recent_context: self.recent_context.as_deref(),
        };

        // System turns don't participate in interrupts — use a dead-end channel
        let mut sys_interrupt_rx = interrupt::dead_interrupt_rx();

        // System turns don't inject time context (no user-facing timestamps)
        let texts = execute_turn(
            provider,
            &self.tools,
            &self.tool_filter,
            &self.mcp_registry,
            &self.identity,
            &self.options,
            &memory_ctx,
            prompt_ctx,
            &mut thread_messages,
            display,
            None,
            &mut sys_interrupt_rx,
        )
        .await?;

        let response = texts.last().cloned().unwrap_or_default();

        Ok(SystemTurnResult {
            response,
            messages: thread_messages.messages().to_vec(),
        })
    }

    /// Compute an approximate token usage summary for the current agent context.
    #[must_use]
    pub fn context_summary(&self) -> context::ContextSummary {
        let memory_ctx = MemoryContext {
            observations: self.observations.as_deref(),
            recent_context: self.recent_context.as_deref(),
        };
        context::compute_context_summary(&self.identity, &memory_ctx, &self.recent_messages)
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

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "test code uses unwrap and indexing for clarity"
)]
mod tests {
    use super::turn::MAX_TOOL_ITERATIONS;
    use super::*;
    use crate::channels::null::NullDisplay;
    use crate::mcp::McpRegistry;
    use crate::models::{ModelError, ModelResponse, ToolCall, ToolDefinition};
    use crate::tools::{FileTracker, PathPolicy, ToolFilter};
    use async_trait::async_trait;
    use std::collections::HashSet;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn no_filter() -> SharedToolFilter {
        ToolFilter::new_shared(HashSet::new())
    }

    fn empty_mcp() -> SharedMcpRegistry {
        McpRegistry::new_shared()
    }

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
            no_filter(),
            empty_mcp(),
            IdentityFiles::default(),
            CompletionOptions::default(),
            chrono_tz::UTC,
            std::path::PathBuf::from("/tmp/ironclaw-test-inbox"),
        );

        let display = NullDisplay;
        let mut irx = interrupt::dead_interrupt_rx();
        let result = agent
            .process_message("hi", &display, None, &PromptContext::none(), &mut irx)
            .await
            .unwrap();
        assert_eq!(result, vec!["hello there"], "should return model text");
    }

    #[tokio::test]
    async fn tool_loop_then_text() {
        let mut registry = ToolRegistry::new();
        registry.register_defaults(
            FileTracker::new_shared(),
            PathPolicy::new_shared(std::path::PathBuf::from("/tmp")),
        );

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
            no_filter(),
            empty_mcp(),
            IdentityFiles::default(),
            CompletionOptions::default(),
            chrono_tz::UTC,
            std::path::PathBuf::from("/tmp/ironclaw-test-inbox"),
        );

        let display = NullDisplay;
        let mut irx = interrupt::dead_interrupt_rx();
        let result = agent
            .process_message(
                "run echo test",
                &display,
                None,
                &PromptContext::none(),
                &mut irx,
            )
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
        registry.register_defaults(
            FileTracker::new_shared(),
            PathPolicy::new_shared(std::path::PathBuf::from("/tmp")),
        );

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
            no_filter(),
            empty_mcp(),
            IdentityFiles::default(),
            CompletionOptions::default(),
            chrono_tz::UTC,
            std::path::PathBuf::from("/tmp/ironclaw-test-inbox"),
        );

        let display = NullDisplay;
        let mut irx = interrupt::dead_interrupt_rx();
        let result = agent
            .process_message(
                "what does echo test print?",
                &display,
                None,
                &PromptContext::none(),
                &mut irx,
            )
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
        registry.register_defaults(
            FileTracker::new_shared(),
            PathPolicy::new_shared(std::path::PathBuf::from("/tmp")),
        );

        let provider = MockProvider::new(responses);
        let mut agent = Agent::new(
            Box::new(provider),
            registry,
            no_filter(),
            empty_mcp(),
            IdentityFiles::default(),
            CompletionOptions::default(),
            chrono_tz::UTC,
            std::path::PathBuf::from("/tmp/ironclaw-test-inbox"),
        );

        let display = NullDisplay;
        let mut irx = interrupt::dead_interrupt_rx();
        let result = agent
            .process_message(
                "loop forever",
                &display,
                None,
                &PromptContext::none(),
                &mut irx,
            )
            .await;
        assert!(result.is_err(), "should error after max iterations");
    }

    #[tokio::test]
    async fn run_system_turn_ephemeral() {
        let provider =
            MockProvider::new(vec![ModelResponse::new("HEARTBEAT_OK".to_string(), vec![])]);

        let agent = Agent::new(
            Box::new(provider),
            ToolRegistry::new(),
            no_filter(),
            empty_mcp(),
            IdentityFiles::default(),
            CompletionOptions::default(),
            chrono_tz::UTC,
            std::path::PathBuf::from("/tmp/ironclaw-test-inbox"),
        );

        let display = NullDisplay;
        let result = agent
            .run_system_turn("check status", &display, None, &PromptContext::none())
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
            no_filter(),
            empty_mcp(),
            IdentityFiles::default(),
            CompletionOptions::default(),
            chrono_tz::UTC,
            std::path::PathBuf::from("/tmp/ironclaw-test-inbox"),
        );

        agent.queue_system_event("email arrived from boss".to_string());
        let display = NullDisplay;
        let mut irx = interrupt::dead_interrupt_rx();
        agent
            .process_message(
                "what's up?",
                &display,
                None,
                &PromptContext::none(),
                &mut irx,
            )
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
            no_filter(),
            empty_mcp(),
            IdentityFiles::default(),
            CompletionOptions::default(),
            chrono_tz::UTC,
            std::path::PathBuf::from("/tmp/ironclaw-test-inbox"),
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

    type InjectEntry = (usize, Vec<interrupt::Interrupt>);

    /// Provider that captures messages per call and sends interrupts after call N.
    struct CapturingProvider {
        responses: Vec<ModelResponse>,
        call_count: Arc<AtomicUsize>,
        /// Messages seen by the provider on each call (indexed by call number).
        captured: Arc<tokio::sync::Mutex<Vec<Vec<Message>>>>,
        /// Interrupts to send after a given call index: `(call_index, interrupts)`.
        inject_after: Arc<tokio::sync::Mutex<Vec<InjectEntry>>>,
        interrupt_tx: tokio::sync::mpsc::Sender<interrupt::Interrupt>,
    }

    impl CapturingProvider {
        fn new(
            responses: Vec<ModelResponse>,
            interrupt_tx: tokio::sync::mpsc::Sender<interrupt::Interrupt>,
        ) -> Self {
            Self {
                responses,
                call_count: Arc::new(AtomicUsize::new(0)),
                captured: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                inject_after: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                interrupt_tx,
            }
        }

        fn schedule_interrupt(&self, after_call: usize, interrupts: Vec<interrupt::Interrupt>) {
            // Block on mutex — only called from test setup, not async context
            self.inject_after
                .try_lock()
                .unwrap()
                .push((after_call, interrupts));
        }
    }

    #[async_trait]
    impl ModelProvider for CapturingProvider {
        async fn complete(
            &self,
            messages: &[Message],
            _tools: &[ToolDefinition],
            _options: &CompletionOptions,
        ) -> Result<ModelResponse, ModelError> {
            let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
            self.captured.lock().await.push(messages.to_vec());

            let response = self
                .responses
                .get(idx)
                .cloned()
                .ok_or_else(|| ModelError::Api("no more mock responses".to_string()))?;

            // Inject scheduled interrupts after this call
            let scheduled: Vec<_> = {
                let guard = self.inject_after.lock().await;
                guard
                    .iter()
                    .filter(|(after, _)| *after == idx)
                    .flat_map(|(_, ints)| ints.clone())
                    .collect()
            };
            for intr in scheduled {
                drop(self.interrupt_tx.try_send(intr));
            }

            Ok(response)
        }

        fn model_name(&self) -> &'static str {
            "capturing-mock"
        }
    }

    fn make_inbound(id: &str, content: &str) -> crate::channels::types::InboundMessage {
        crate::channels::types::InboundMessage {
            id: id.to_string(),
            content: content.to_string(),
            origin: crate::channels::types::MessageOrigin {
                channel: "test".to_string(),
                sender_name: "tester".to_string(),
                sender_id: "t1".to_string(),
            },
            timestamp: chrono::Utc::now(),
        }
    }

    #[tokio::test]
    async fn interrupt_injects_user_message_mid_turn() {
        let mut registry = ToolRegistry::new();
        registry.register_defaults(
            FileTracker::new_shared(),
            PathPolicy::new_shared(std::path::PathBuf::from("/tmp")),
        );

        let (interrupt_tx, mut interrupt_rx) = tokio::sync::mpsc::channel(32);

        let provider = CapturingProvider::new(
            vec![
                // Call 0: tool call — triggers tool loop iteration
                ModelResponse::new(
                    String::new(),
                    vec![ToolCall {
                        id: "call_1".to_string(),
                        name: "exec".to_string(),
                        arguments: serde_json::json!({"command": "echo test"}),
                    }],
                ),
                // Call 1: final text
                ModelResponse::new("done".to_string(), vec![]),
            ],
            interrupt_tx,
        );
        provider.schedule_interrupt(
            0,
            vec![interrupt::Interrupt::UserMessage(make_inbound(
                "int-1",
                "actually, do X instead",
            ))],
        );
        let captured = Arc::clone(&provider.captured);

        let mut agent = Agent::new(
            Box::new(provider),
            registry,
            no_filter(),
            empty_mcp(),
            IdentityFiles::default(),
            CompletionOptions::default(),
            chrono_tz::UTC,
            std::path::PathBuf::from("/tmp/ironclaw-test-inbox"),
        );

        let display = NullDisplay;
        let result = agent
            .process_message(
                "hello",
                &display,
                None,
                &PromptContext::none(),
                &mut interrupt_rx,
            )
            .await
            .unwrap();
        assert_eq!(result, vec!["done"]);

        // The second LLM call (index 1) should contain the injected user message
        let calls = captured.lock().await;
        assert_eq!(calls.len(), 2, "provider should be called twice");
        let second_call_msgs = &calls[1];
        assert!(
            second_call_msgs
                .iter()
                .any(|m| m.content.contains("actually, do X instead")),
            "second call should contain the injected user message"
        );
    }

    #[tokio::test]
    async fn multiple_interrupts_drained_at_checkpoint() {
        let mut registry = ToolRegistry::new();
        registry.register_defaults(
            FileTracker::new_shared(),
            PathPolicy::new_shared(std::path::PathBuf::from("/tmp")),
        );

        let (interrupt_tx, mut interrupt_rx) = tokio::sync::mpsc::channel(32);

        let provider = CapturingProvider::new(
            vec![
                ModelResponse::new(
                    String::new(),
                    vec![ToolCall {
                        id: "call_1".to_string(),
                        name: "exec".to_string(),
                        arguments: serde_json::json!({"command": "echo test"}),
                    }],
                ),
                ModelResponse::new("done".to_string(), vec![]),
            ],
            interrupt_tx,
        );
        provider.schedule_interrupt(
            0,
            vec![
                interrupt::Interrupt::UserMessage(make_inbound("int-1", "first steering")),
                interrupt::Interrupt::UserMessage(make_inbound("int-2", "second steering")),
            ],
        );
        let captured = Arc::clone(&provider.captured);

        let mut agent = Agent::new(
            Box::new(provider),
            registry,
            no_filter(),
            empty_mcp(),
            IdentityFiles::default(),
            CompletionOptions::default(),
            chrono_tz::UTC,
            std::path::PathBuf::from("/tmp/ironclaw-test-inbox"),
        );

        let display = NullDisplay;
        agent
            .process_message(
                "hello",
                &display,
                None,
                &PromptContext::none(),
                &mut interrupt_rx,
            )
            .await
            .unwrap();

        let calls = captured.lock().await;
        let second_call_msgs = &calls[1];
        let has_first = second_call_msgs
            .iter()
            .any(|m| m.content.contains("first steering"));
        let has_second = second_call_msgs
            .iter()
            .any(|m| m.content.contains("second steering"));
        assert!(
            has_first && has_second,
            "both interrupts should appear in the second call"
        );
    }

    #[tokio::test]
    async fn interrupt_during_final_response_not_consumed() {
        let (interrupt_tx, mut interrupt_rx) = tokio::sync::mpsc::channel(32);

        let provider = CapturingProvider::new(
            vec![
                // Single call: returns final text (no tool calls)
                ModelResponse::new("final answer".to_string(), vec![]),
            ],
            interrupt_tx.clone(),
        );
        // Schedule an interrupt during the first (and only) call
        provider.schedule_interrupt(
            0,
            vec![interrupt::Interrupt::UserMessage(make_inbound(
                "int-late",
                "too late to steer",
            ))],
        );

        let mut agent = Agent::new(
            Box::new(provider),
            ToolRegistry::new(),
            no_filter(),
            empty_mcp(),
            IdentityFiles::default(),
            CompletionOptions::default(),
            chrono_tz::UTC,
            std::path::PathBuf::from("/tmp/ironclaw-test-inbox"),
        );

        let display = NullDisplay;
        let result = agent
            .process_message(
                "hello",
                &display,
                None,
                &PromptContext::none(),
                &mut interrupt_rx,
            )
            .await
            .unwrap();
        assert_eq!(result, vec!["final answer"]);

        // The interrupt should still be in the channel — not consumed
        let pending = interrupt_rx.try_recv();
        assert!(
            pending.is_ok(),
            "interrupt should remain in the channel for the next turn"
        );
    }

    #[tokio::test]
    async fn empty_response_returns_error() {
        let provider = MockProvider::new(vec![ModelResponse::new(String::new(), vec![])]);

        let mut agent = Agent::new(
            Box::new(provider),
            ToolRegistry::new(),
            no_filter(),
            empty_mcp(),
            IdentityFiles::default(),
            CompletionOptions::default(),
            chrono_tz::UTC,
            std::path::PathBuf::from("/tmp/ironclaw-test-inbox"),
        );

        let display = NullDisplay;
        let mut irx = interrupt::dead_interrupt_rx();
        let result = agent
            .process_message("hello", &display, None, &PromptContext::none(), &mut irx)
            .await;
        assert!(result.is_err(), "empty response should return error");

        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("empty response"),
            "error should mention empty response, got: {err_msg}"
        );
    }
}
