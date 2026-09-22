//! Agent struct, configuration, and turn dispatch.

use tokio_util::sync::CancellationToken;

use crate::bus::{EndpointName, Publisher};
use crate::inference::{CompletionOptions, InferenceProvider, Message};
use crate::interfaces::types::MessageOrigin;
use crate::mcp::SharedMcpRegistry;
use crate::tools::ToolRegistry;
use crate::workspace::identity::IdentityFiles;

use super::context::{MemoryContext, PromptContext, StatusLine};
use super::hop::HopCounter;
use super::interrupt;
use super::recent_messages::RecentMessages;
use super::turn::{EventContext, EventTarget, TurnResources, execute_turn};

/// Configuration for creating a new `Agent`.
pub struct AgentConfig {
    pub options: CompletionOptions,
    pub tz: chrono_tz::Tz,
    /// Workspace layout used to reload identity files from disk each turn.
    ///
    /// `None` pins the agent to the identity snapshot passed at construction
    /// (used by tests that don't touch the filesystem).
    pub layout: Option<crate::workspace::layout::WorkspaceLayout>,
}

/// The agent runtime that processes user messages through the model.
pub struct Agent {
    provider: Box<dyn InferenceProvider>,
    tools: ToolRegistry,
    mcp_registry: SharedMcpRegistry,
    /// Last-known-good identity snapshot. Refreshed from disk at each turn entry
    /// via [`Agent::load_identity_snapshot`]; retained as the fallback when a
    /// reload fails.
    identity: IdentityFiles,
    recent_messages: RecentMessages,
    options: CompletionOptions,
    observations: Option<String>,
    /// Narrative summary from the most recent observation cycle.
    recent_context: Option<String>,
    tz: chrono_tz::Tz,
    /// Workspace layout for per-turn identity reloads. `None` pins identity to
    /// the construction-time snapshot (test constructors).
    layout: Option<crate::workspace::layout::WorkspaceLayout>,
    last_user_message_at: Option<chrono::NaiveDateTime>,
    /// Main's current-turn hop count: the highest hop count among the
    /// inputs (kickoff message, agent-message interrupts drained mid-turn)
    /// driving whichever turn is currently running. Shared with the
    /// `message_agent`/`subagent_spawn` tools registered against this agent,
    /// so they can compute the hop count an outgoing message or spawn
    /// carries without a separate side channel.
    hop_counter: HopCounter,
}

impl Agent {
    /// Create a new agent with the given components.
    #[must_use]
    pub fn new(
        provider: Box<dyn InferenceProvider>,
        tools: ToolRegistry,
        mcp_registry: SharedMcpRegistry,
        identity: IdentityFiles,
        config: AgentConfig,
        hop_counter: HopCounter,
    ) -> Self {
        Self {
            provider,
            tools,
            mcp_registry,
            identity,
            recent_messages: RecentMessages::new(),
            options: config.options,
            observations: None,
            recent_context: None,
            tz: config.tz,
            layout: config.layout,
            last_user_message_at: None,
            hop_counter,
        }
    }

    /// Load a fresh identity snapshot from disk. On read failure, logs an
    /// error and falls back to the last-known-good snapshot.
    async fn load_identity_snapshot(&self) -> IdentityFiles {
        let Some(layout) = &self.layout else {
            return self.identity.clone();
        };
        match IdentityFiles::load(layout).await {
            Ok(identity) => identity,
            Err(e) => {
                tracing::error!(error = %e, "identity reload failed; using last-known-good snapshot from previous turn");
                self.identity.clone()
            }
        }
    }

    /// Get a reference to the MCP registry.
    #[must_use]
    pub fn mcp_registry(&self) -> &SharedMcpRegistry {
        &self.mcp_registry
    }

    /// This agent's current-turn hop counter. Interior-mutable (an `Arc`
    /// around an atomic), so the gateway event loop updates it directly
    /// through this shared reference as a turn starts and as agent-message
    /// interrupts arrive mid-turn, and the `message_agent`/`subagent_spawn`
    /// tools (holding their own clone from construction time) read the same
    /// value when computing an outgoing hop count.
    #[must_use]
    pub fn hop_counter(&self) -> &HopCounter {
        &self.hop_counter
    }

    /// Replace the model provider and completion options in-place (e.g. after a config reload).
    pub fn swap_provider(
        &mut self,
        provider: Box<dyn InferenceProvider>,
        options: CompletionOptions,
    ) {
        tracing::info!(
            old_model = self.provider.model_name(),
            new_model = provider.model_name(),
            "swapping model provider"
        );
        self.provider = provider;
        self.options = options;
    }

    /// Reload the `ollama_web_search` tool in place from the current
    /// standalone web search backend config (e.g. after a config reload).
    ///
    /// Mirrors `gateway::startup::tools::init_tool_registry`'s startup-time
    /// gating: removes any existing `ollama_web_search` tool, then re-adds
    /// it with the current API key/base URL if `backend` names the
    /// `"ollama"` backend. A no-op call (backend unchanged) still removes
    /// and re-adds the tool — harmless, since the tool itself is stateless
    /// beyond the credentials it's constructed with.
    pub fn reload_ollama_web_search_tool(
        &mut self,
        backend: Option<&crate::config::StandaloneBackendConfig>,
    ) {
        let was_registered = self.tools.remove("ollama_web_search");
        if let Some(backend) = backend
            && backend.name == "ollama"
        {
            let base_url = backend
                .base_url
                .clone()
                .unwrap_or_else(|| "https://api.ollama.com".to_string());
            self.tools
                .register_ollama_web_search_tool(backend.api_key.clone(), base_url);
            tracing::info!("reloaded ollama_web_search tool from config");
        } else if was_registered {
            tracing::info!("removed ollama_web_search tool: no longer configured");
        }
    }

    /// Reload observations from the observation log file.
    ///
    /// # Errors
    /// Returns an error if the file exists but cannot be read or parsed.
    pub async fn reload_observations(
        &mut self,
        layout: &crate::workspace::layout::WorkspaceLayout,
    ) -> anyhow::Result<()> {
        self.observations =
            super::context::loading::load_observations(&layout.observations_json()).await?;
        Ok(())
    }

    /// Reload narrative context from the `recent_context.json` file.
    ///
    /// # Errors
    /// Returns an error if the file exists but cannot be parsed.
    pub async fn reload_recent_context(
        &mut self,
        layout: &crate::workspace::layout::WorkspaceLayout,
    ) -> anyhow::Result<()> {
        self.recent_context =
            super::context::loading::load_recent_context_narrative(&layout.recent_context_json())
                .await?;
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

    /// Clear all in-memory messages (used after idle transition + observer).
    pub fn clear_messages(&mut self) {
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

    /// Inject a system message directly into the conversation history.
    ///
    /// Used for background task results that should be immediately visible
    /// in the agent's context rather than waiting for the next user turn.
    pub fn inject_system_message(&mut self, content: impl Into<String>) {
        self.recent_messages.push(Message::system(content));
    }

    /// Inject an inbound user message directly into the conversation history.
    ///
    /// Used for user messages that arrived as interrupts during a turn's final
    /// LLM call and were drained after the turn completed. Ensures the message
    /// (with its sender, images, and background context) is visible in the next
    /// turn without being lost.
    pub fn inject_inbound_message(&mut self, message: crate::interfaces::types::InboundMessage) {
        self.recent_messages.extend(message.into_history_messages());
    }

    /// Build a [`MemoryContext`] from borrowed observation/narrative fields.
    ///
    /// Takes explicit field references rather than `&self` so callers that
    /// also need a simultaneous `&mut self.recent_messages` (e.g. turns that
    /// push a message into the buffer) aren't blocked by a whole-struct
    /// immutable borrow.
    fn memory_ctx<'a>(
        observations: Option<&'a str>,
        recent_context: Option<&'a str>,
    ) -> MemoryContext<'a> {
        MemoryContext {
            observations,
            recent_context,
        }
    }

    /// Build a [`TurnResources`] from borrowed component fields.
    ///
    /// Takes explicit field references rather than `&self` for the same
    /// reason as [`Agent::memory_ctx`].
    fn turn_resources<'a>(
        provider: &'a dyn InferenceProvider,
        tools: &'a ToolRegistry,
        mcp_registry: &'a SharedMcpRegistry,
        identity: &'a IdentityFiles,
        options: &'a CompletionOptions,
        stop_token: &'a CancellationToken,
        hop_counter: &'a HopCounter,
    ) -> TurnResources<'a> {
        TurnResources {
            provider,
            tools,
            mcp_registry,
            identity,
            options,
            stop_token,
            // The main agent persists its transcript separately
            // (`recent_messages.json`, written after the whole turn).
            transcript_sink: None,
            hop_counter,
        }
    }

    /// Process a user message through the model, executing tool calls as needed.
    ///
    /// Returns a vec containing the final text-only response. Intermediate texts
    /// emitted alongside tool calls are sent via `reply` in real-time but not
    /// included in the return value.
    ///
    /// # Errors
    /// Returns an error if the model call fails or tool execution errors
    /// are unrecoverable.
    #[expect(
        clippy::too_many_arguments,
        reason = "publisher and topic params added during bus migration"
    )]
    #[tracing::instrument(skip_all)]
    pub async fn process_message(
        &mut self,
        user_input: &str,
        publisher: &Publisher,
        output_endpoint: Option<&EndpointName>,
        tool_activity_endpoint: Option<&EndpointName>,
        correlation_id: &str,
        origin: Option<&MessageOrigin>,
        prompt_ctx: &PromptContext<'_>,
        interrupt_rx: &mut tokio::sync::mpsc::Receiver<interrupt::Interrupt>,
        images: &[crate::inference::ImageData],
        subconscious: Option<&crate::subconscious::SubconsciousWatch>,
        stop_token: &CancellationToken,
    ) -> anyhow::Result<Vec<String>> {
        tracing::debug!("processing user message");
        self.identity = self.load_identity_snapshot().await;
        let now = crate::time::now_local(self.tz);
        let status_line = StatusLine {
            now,
            last_message_at: self.last_user_message_at,
            message_source: origin.map(|o| o.endpoint.clone()),
        };
        self.last_user_message_at = Some(now);

        let sender = origin.and_then(|o| o.sender.clone());
        if images.is_empty() {
            self.recent_messages
                .push(Message::user(user_input).with_sender(sender));
        } else {
            self.recent_messages
                .push(Message::user_with_images(user_input, images.to_vec()).with_sender(sender));
        }

        let memory_ctx =
            Self::memory_ctx(self.observations.as_deref(), self.recent_context.as_deref());
        let resources = Self::turn_resources(
            &*self.provider,
            &self.tools,
            &self.mcp_registry,
            &self.identity,
            &self.options,
            stop_token,
            &self.hop_counter,
        );
        let events = EventContext {
            publisher,
            target: EventTarget::Endpoint {
                output_endpoint,
                tool_activity_endpoint,
                correlation_id,
            },
            session_conversation: None,
        };
        execute_turn(
            &resources,
            &memory_ctx,
            prompt_ctx,
            &mut self.recent_messages,
            &events,
            Some(&status_line),
            interrupt_rx,
            subconscious,
        )
        .await
    }

    /// Compute a per-section token breakdown for the current agent context.
    pub async fn context_breakdown(
        &self,
        prompt_ctx: &PromptContext<'_>,
    ) -> super::context::ContextBreakdown {
        // Reload identity so `/context` reflects on-disk edits, not the snapshot
        // from the last turn.
        let identity = self.load_identity_snapshot().await;
        let memory_ctx =
            Self::memory_ctx(self.observations.as_deref(), self.recent_context.as_deref());

        let builtin_defs = self.tools.definitions();

        let mcp_defs = self.mcp_registry.read().await.tool_definitions();

        let token_count = |def: &crate::inference::ToolDefinition| {
            let param_str = def.parameters.to_string();
            crate::memory::tokens::estimate_tokens(&def.name)
                + crate::memory::tokens::estimate_tokens(&def.description)
                + crate::memory::tokens::estimate_tokens(&param_str)
        };
        let system_tool_tokens: usize = builtin_defs.iter().map(token_count).sum();
        let mcp_tool_tokens: usize = mcp_defs.iter().map(token_count).sum();

        super::context::compute_context_breakdown(
            &identity,
            &memory_ctx,
            prompt_ctx,
            &self.recent_messages,
            system_tool_tokens,
            mcp_tool_tokens,
        )
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
    clippy::indexing_slicing,
    reason = "test code uses indexing for clarity"
)]
mod tests {
    use super::super::turn::MAX_TOOL_ITERATIONS;
    use super::*;
    use crate::bus;
    use crate::inference::{InferenceError, InferenceResponse, ToolCall, ToolDefinition};
    use crate::mcp::McpRegistry;
    use crate::tools::{FileTracker, PathPolicy};
    use async_trait::async_trait;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn empty_mcp() -> SharedMcpRegistry {
        McpRegistry::new_shared()
    }

    /// Create a test publisher and endpoint for bus-based tests.
    fn test_bus() -> (Publisher, bus::EndpointName) {
        let handle = bus::spawn_broker();
        let publisher = handle.publisher();
        let ep = bus::EndpointName::from("test");
        (publisher, ep)
    }

    /// Mock provider that returns pre-configured responses in sequence.
    ///
    /// Intentionally duplicated across agent, observer, and reflector tests — each mock
    /// has slightly different fields. Extract a shared mock when a 4th instance appears.
    struct MockProvider {
        responses: Vec<InferenceResponse>,
        call_count: Arc<AtomicUsize>,
    }

    impl MockProvider {
        fn new(responses: Vec<InferenceResponse>) -> Self {
            Self {
                responses,
                call_count: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    #[async_trait]
    impl InferenceProvider for MockProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _options: &CompletionOptions,
        ) -> Result<InferenceResponse, InferenceError> {
            let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
            self.responses
                .get(idx)
                .cloned()
                .ok_or_else(|| InferenceError::Api("no more mock responses".to_string()))
        }

        fn model_name(&self) -> &'static str {
            "mock-model"
        }
    }

    #[tokio::test]
    async fn single_text_response() {
        let provider = MockProvider::new(vec![InferenceResponse::new(
            "hello there".to_string(),
            vec![],
        )]);

        let mut agent = Agent::new(
            Box::new(provider),
            ToolRegistry::new(),
            empty_mcp(),
            IdentityFiles::default(),
            AgentConfig {
                options: CompletionOptions::default(),
                tz: chrono_tz::UTC,
                layout: None,
            },
            crate::agent::HopCounter::new(0),
        );

        let (publisher, ep) = test_bus();
        let mut irx = interrupt::dead_interrupt_rx();
        let result = agent
            .process_message(
                "hi",
                &publisher,
                Some(&ep),
                None,
                "",
                None,
                &PromptContext::default(),
                &mut irx,
                &[],
                None,
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(result, vec!["hello there"], "should return model text");
    }

    #[tokio::test]
    async fn tool_loop_then_text() {
        let mut registry = ToolRegistry::new();
        registry.register_defaults(FileTracker::new_shared(), PathPolicy::new_shared());

        let provider = MockProvider::new(vec![
            InferenceResponse::new(
                String::new(),
                vec![ToolCall {
                    id: "call_1".to_string(),
                    name: "exec".to_string(),
                    arguments: serde_json::json!({"command": "echo test"}),
                }],
            ),
            InferenceResponse::new("the result was: test".to_string(), vec![]),
        ]);

        let mut agent = Agent::new(
            Box::new(provider),
            registry,
            empty_mcp(),
            IdentityFiles::default(),
            AgentConfig {
                options: CompletionOptions::default(),
                tz: chrono_tz::UTC,
                layout: None,
            },
            crate::agent::HopCounter::new(0),
        );

        let (publisher, ep) = test_bus();
        let mut irx = interrupt::dead_interrupt_rx();
        let result = agent
            .process_message(
                "run echo test",
                &publisher,
                Some(&ep),
                None,
                "",
                None,
                &PromptContext::default(),
                &mut irx,
                &[],
                None,
                &CancellationToken::new(),
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
    async fn intermediate_text_not_in_return_value() {
        let mut registry = ToolRegistry::new();
        registry.register_defaults(FileTracker::new_shared(), PathPolicy::new_shared());

        // First response has text alongside tool calls (intermediate), second is final.
        let provider = MockProvider::new(vec![
            InferenceResponse::new(
                "Let me check that for you...".to_string(),
                vec![ToolCall {
                    id: "call_1".to_string(),
                    name: "exec".to_string(),
                    arguments: serde_json::json!({"command": "echo test"}),
                }],
            ),
            InferenceResponse::new("Done! The output was: test".to_string(), vec![]),
        ]);

        let mut agent = Agent::new(
            Box::new(provider),
            registry,
            empty_mcp(),
            IdentityFiles::default(),
            AgentConfig {
                options: CompletionOptions::default(),
                tz: chrono_tz::UTC,
                layout: None,
            },
            crate::agent::HopCounter::new(0),
        );

        let (publisher, ep) = test_bus();
        let mut irx = interrupt::dead_interrupt_rx();
        let result = agent
            .process_message(
                "what does echo test print?",
                &publisher,
                Some(&ep),
                None,
                "",
                None,
                &PromptContext::default(),
                &mut irx,
                &[],
                None,
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(
            result,
            vec!["Done! The output was: test"],
            "should return only final text, intermediate is sent via reply handle"
        );
    }

    #[tokio::test]
    async fn max_iterations_guard() {
        let responses: Vec<InferenceResponse> = (0..=MAX_TOOL_ITERATIONS)
            .map(|i| {
                InferenceResponse::new(
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
        registry.register_defaults(FileTracker::new_shared(), PathPolicy::new_shared());

        let provider = MockProvider::new(responses);
        let mut agent = Agent::new(
            Box::new(provider),
            registry,
            empty_mcp(),
            IdentityFiles::default(),
            AgentConfig {
                options: CompletionOptions::default(),
                tz: chrono_tz::UTC,
                layout: None,
            },
            crate::agent::HopCounter::new(0),
        );

        let (publisher, ep) = test_bus();
        let mut irx = interrupt::dead_interrupt_rx();
        let result = agent
            .process_message(
                "loop forever",
                &publisher,
                Some(&ep),
                None,
                "",
                None,
                &PromptContext::default(),
                &mut irx,
                &[],
                None,
                &CancellationToken::new(),
            )
            .await;
        assert!(result.is_err(), "should error after max iterations");
    }

    #[test]
    fn inject_inbound_message_appears_in_history() {
        let mut agent = Agent::new(
            Box::new(MockProvider::new(vec![])),
            ToolRegistry::new(),
            empty_mcp(),
            IdentityFiles::default(),
            AgentConfig {
                options: CompletionOptions::default(),
                tz: chrono_tz::UTC,
                layout: None,
            },
            crate::agent::HopCounter::new(0),
        );

        agent.inject_inbound_message(make_inbound("m1", "leftover interrupt message"));

        let msgs = agent.messages_since(0);
        assert_eq!(msgs.len(), 1, "should have one user message");
        assert_eq!(
            msgs[0].content, "leftover interrupt message",
            "user message content should match"
        );
        assert_eq!(
            msgs[0].role,
            crate::inference::Role::User,
            "injected message should have User role"
        );
    }

    #[test]
    fn inject_system_message_appears_in_history() {
        let mut agent = Agent::new(
            Box::new(MockProvider::new(vec![])),
            ToolRegistry::new(),
            empty_mcp(),
            IdentityFiles::default(),
            AgentConfig {
                options: CompletionOptions::default(),
                tz: chrono_tz::UTC,
                layout: None,
            },
            crate::agent::HopCounter::new(0),
        );

        agent.inject_system_message("background task completed: report-gen");

        let msgs = agent.messages_since(0);
        assert_eq!(msgs.len(), 1, "should have one system message");
        assert_eq!(
            msgs[0].content, "background task completed: report-gen",
            "system message content should match"
        );
        assert_eq!(
            msgs[0].role,
            crate::inference::Role::System,
            "injected message should have System role"
        );
    }

    #[test]
    fn injected_message_excluded_from_later_messages_since() {
        let mut agent = Agent::new(
            Box::new(MockProvider::new(vec![])),
            ToolRegistry::new(),
            empty_mcp(),
            IdentityFiles::default(),
            AgentConfig {
                options: CompletionOptions::default(),
                tz: chrono_tz::UTC,
                layout: None,
            },
            crate::agent::HopCounter::new(0),
        );

        // Inject a background message before the "turn"
        agent.inject_system_message("bg result arrived");

        // Snapshot count after injection — simulates `before = agent.message_count()`
        let before = agent.message_count();
        assert_eq!(before, 1, "injected message should be counted");

        // Simulate a user turn by pushing user + assistant messages
        agent.recent_messages.push(Message::user("hello"));
        agent
            .recent_messages
            .push(Message::assistant("hi there", None));

        // messages_since(before) should only contain the turn's messages
        let turn_msgs = agent.messages_since(before);
        assert_eq!(
            turn_msgs.len(),
            2,
            "only the user turn messages should appear after the snapshot"
        );
        assert_eq!(turn_msgs[0].content, "hello");
        assert_eq!(turn_msgs[1].content, "hi there");
    }

    #[test]
    fn rotate_messages_retains_last_exchanges() {
        let mut agent = Agent::new(
            Box::new(MockProvider::new(vec![])),
            ToolRegistry::new(),
            empty_mcp(),
            IdentityFiles::default(),
            AgentConfig {
                options: CompletionOptions::default(),
                tz: chrono_tz::UTC,
                layout: None,
            },
            crate::agent::HopCounter::new(0),
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
        assert_eq!(msgs[2].content, "question 3");
        assert_eq!(msgs[3].content, "answer 3");
        assert_eq!(msgs[4].content, "question 4");
        assert_eq!(
            msgs[5].content, "answer 4",
            "last retained should be exchange 4"
        );
    }

    #[test]
    fn clear_messages_empties_buffer() {
        let mut agent = Agent::new(
            Box::new(MockProvider::new(vec![])),
            ToolRegistry::new(),
            empty_mcp(),
            IdentityFiles::default(),
            AgentConfig {
                options: CompletionOptions::default(),
                tz: chrono_tz::UTC,
                layout: None,
            },
            crate::agent::HopCounter::new(0),
        );
        agent.inject_inbound_message(make_inbound("m1", "hello"));
        agent.inject_inbound_message(make_inbound("m2", "world"));
        assert_eq!(agent.message_count(), 2);
        agent.clear_messages();
        assert_eq!(agent.message_count(), 0);
    }

    type InjectEntry = (usize, Vec<interrupt::Interrupt>);

    /// Provider that captures messages per call and sends interrupts after call N.
    struct CapturingProvider {
        responses: Vec<InferenceResponse>,
        call_count: Arc<AtomicUsize>,
        /// Messages seen by the provider on each call (indexed by call number).
        captured: Arc<tokio::sync::Mutex<Vec<Vec<Message>>>>,
        /// Interrupts to send after a given call index: `(call_index, interrupts)`.
        inject_after: Arc<tokio::sync::Mutex<Vec<InjectEntry>>>,
        interrupt_tx: tokio::sync::mpsc::Sender<interrupt::Interrupt>,
    }

    impl CapturingProvider {
        fn new(
            responses: Vec<InferenceResponse>,
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
    impl InferenceProvider for CapturingProvider {
        async fn complete(
            &self,
            messages: &[Message],
            _tools: &[ToolDefinition],
            _options: &CompletionOptions,
        ) -> Result<InferenceResponse, InferenceError> {
            let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
            self.captured.lock().await.push(messages.to_vec());

            let response = self
                .responses
                .get(idx)
                .cloned()
                .ok_or_else(|| InferenceError::Api("no more mock responses".to_string()))?;

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

    fn make_inbound(id: &str, content: &str) -> crate::interfaces::types::InboundMessage {
        crate::interfaces::types::InboundMessage {
            id: id.to_string(),
            content: content.to_string(),
            origin: crate::interfaces::types::MessageOrigin {
                endpoint: "test".to_string(),
                sender: None,
                conversation: None,
                agent_sender: None,
            },
            timestamp: chrono::Utc::now(),
            images: vec![],
            context: None,
        }
    }

    #[tokio::test]
    async fn interrupt_injects_user_message_mid_turn() {
        let mut registry = ToolRegistry::new();
        registry.register_defaults(FileTracker::new_shared(), PathPolicy::new_shared());

        let (interrupt_tx, mut interrupt_rx) = tokio::sync::mpsc::channel(32);

        let provider = CapturingProvider::new(
            vec![
                // Call 0: tool call — triggers tool loop iteration
                InferenceResponse::new(
                    String::new(),
                    vec![ToolCall {
                        id: "call_1".to_string(),
                        name: "exec".to_string(),
                        arguments: serde_json::json!({"command": "echo test"}),
                    }],
                ),
                // Call 1: final text
                InferenceResponse::new("done".to_string(), vec![]),
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
            empty_mcp(),
            IdentityFiles::default(),
            AgentConfig {
                options: CompletionOptions::default(),
                tz: chrono_tz::UTC,
                layout: None,
            },
            crate::agent::HopCounter::new(0),
        );

        let (publisher, ep) = test_bus();
        let result = agent
            .process_message(
                "hello",
                &publisher,
                Some(&ep),
                None,
                "",
                None,
                &PromptContext::default(),
                &mut interrupt_rx,
                &[],
                None,
                &CancellationToken::new(),
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
        registry.register_defaults(FileTracker::new_shared(), PathPolicy::new_shared());

        let (interrupt_tx, mut interrupt_rx) = tokio::sync::mpsc::channel(32);

        let provider = CapturingProvider::new(
            vec![
                InferenceResponse::new(
                    String::new(),
                    vec![ToolCall {
                        id: "call_1".to_string(),
                        name: "exec".to_string(),
                        arguments: serde_json::json!({"command": "echo test"}),
                    }],
                ),
                InferenceResponse::new("done".to_string(), vec![]),
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
            empty_mcp(),
            IdentityFiles::default(),
            AgentConfig {
                options: CompletionOptions::default(),
                tz: chrono_tz::UTC,
                layout: None,
            },
            crate::agent::HopCounter::new(0),
        );

        let (publisher, ep) = test_bus();
        agent
            .process_message(
                "hello",
                &publisher,
                Some(&ep),
                None,
                "",
                None,
                &PromptContext::default(),
                &mut interrupt_rx,
                &[],
                None,
                &CancellationToken::new(),
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
                InferenceResponse::new("final answer".to_string(), vec![]),
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
            empty_mcp(),
            IdentityFiles::default(),
            AgentConfig {
                options: CompletionOptions::default(),
                tz: chrono_tz::UTC,
                layout: None,
            },
            crate::agent::HopCounter::new(0),
        );

        let (publisher, ep) = test_bus();
        let result = agent
            .process_message(
                "hello",
                &publisher,
                Some(&ep),
                None,
                "",
                None,
                &PromptContext::default(),
                &mut interrupt_rx,
                &[],
                None,
                &CancellationToken::new(),
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
    async fn stop_token_cancelled_before_call_aborts_without_invoking_provider() {
        // A pre-cancelled token means `tokio::select!`'s biased cancellation
        // arm wins before the provider is ever polled — this isolates the
        // "abort the in-flight model call" path from the tool-loop
        // checkpoint path (covered by the drain_interrupts tests in turn.rs
        // and by `stopped_interrupt_ends_turn_after_tool_completes` below).
        let provider = MockProvider::new(vec![InferenceResponse::new(
            "never seen".to_string(),
            vec![],
        )]);
        let call_count = Arc::clone(&provider.call_count);

        let mut agent = Agent::new(
            Box::new(provider),
            ToolRegistry::new(),
            empty_mcp(),
            IdentityFiles::default(),
            AgentConfig {
                options: CompletionOptions::default(),
                tz: chrono_tz::UTC,
                layout: None,
            },
            crate::agent::HopCounter::new(0),
        );

        let (publisher, ep) = test_bus();
        let mut irx = interrupt::dead_interrupt_rx();
        let stop_token = CancellationToken::new();
        stop_token.cancel();

        let result = agent
            .process_message(
                "hello",
                &publisher,
                Some(&ep),
                None,
                "",
                None,
                &PromptContext::default(),
                &mut irx,
                &[],
                None,
                &stop_token,
            )
            .await
            .unwrap();

        assert_eq!(
            result,
            Vec::<String>::new(),
            "a stopped turn returns no text"
        );
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            0,
            "the model should never be called once the turn is stopped"
        );
        assert!(
            agent
                .messages_since(0)
                .iter()
                .any(|m| m.content.contains("[Stopped]")),
            "a stop note should be recorded in history"
        );
    }

    #[tokio::test]
    async fn stopped_interrupt_ends_turn_after_tool_completes() {
        // The tool call in response 0 must run to completion before the
        // Interrupt::Stopped queued alongside it is observed — proving a
        // stop mid-tool-execution doesn't sever the tool, only stops the
        // loop at its next checkpoint.
        let mut registry = ToolRegistry::new();
        registry.register_defaults(FileTracker::new_shared(), PathPolicy::new_shared());

        let (interrupt_tx, mut interrupt_rx) = tokio::sync::mpsc::channel(32);
        let provider = CapturingProvider::new(
            vec![
                InferenceResponse::new(
                    String::new(),
                    vec![ToolCall {
                        id: "call_1".to_string(),
                        name: "exec".to_string(),
                        arguments: serde_json::json!({"command": "echo test"}),
                    }],
                ),
                // A second response exists only to fail the test loudly if
                // the loop wrongly calls the model again after the stop.
                InferenceResponse::new("should never be reached".to_string(), vec![]),
            ],
            interrupt_tx,
        );
        provider.schedule_interrupt(0, vec![interrupt::Interrupt::Stopped]);
        let call_count = Arc::clone(&provider.call_count);

        let mut agent = Agent::new(
            Box::new(provider),
            registry,
            empty_mcp(),
            IdentityFiles::default(),
            AgentConfig {
                options: CompletionOptions::default(),
                tz: chrono_tz::UTC,
                layout: None,
            },
            crate::agent::HopCounter::new(0),
        );

        let (publisher, ep) = test_bus();
        let result = agent
            .process_message(
                "hello",
                &publisher,
                Some(&ep),
                None,
                "",
                None,
                &PromptContext::default(),
                &mut interrupt_rx,
                &[],
                None,
                &CancellationToken::new(),
            )
            .await
            .unwrap();

        assert_eq!(
            result,
            Vec::<String>::new(),
            "a stopped turn returns no text"
        );
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            1,
            "the tool's model call runs, but the model is never called again after the stop"
        );
        let messages = agent.messages_since(0);
        assert!(
            messages
                .iter()
                .any(|m| m.role == crate::inference::Role::Tool),
            "the in-flight tool call should still have run to completion"
        );
        assert!(
            messages.iter().any(|m| m.content.contains("[Stopped]")),
            "a stop note should be recorded in history"
        );
    }

    #[tokio::test]
    async fn empty_response_returns_error() {
        let provider = MockProvider::new(vec![
            InferenceResponse::new(String::new(), vec![]),
            InferenceResponse::new(String::new(), vec![]),
            InferenceResponse::new(String::new(), vec![]),
        ]);

        let mut agent = Agent::new(
            Box::new(provider),
            ToolRegistry::new(),
            empty_mcp(),
            IdentityFiles::default(),
            AgentConfig {
                options: CompletionOptions::default(),
                tz: chrono_tz::UTC,
                layout: None,
            },
            crate::agent::HopCounter::new(0),
        );

        let (publisher, ep) = test_bus();
        let mut irx = interrupt::dead_interrupt_rx();
        let result = agent
            .process_message(
                "hello",
                &publisher,
                Some(&ep),
                None,
                "",
                None,
                &PromptContext::default(),
                &mut irx,
                &[],
                None,
                &CancellationToken::new(),
            )
            .await;
        assert!(result.is_err(), "empty response should return error");

        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("empty response"),
            "error should mention empty response, got: {err_msg}"
        );
    }

    #[test]
    fn rotate_messages_retains_all_when_fewer_than_three_exchanges() {
        let mut agent = Agent::new(
            Box::new(MockProvider::new(vec![])),
            ToolRegistry::new(),
            empty_mcp(),
            IdentityFiles::default(),
            AgentConfig {
                options: CompletionOptions::default(),
                tz: chrono_tz::UTC,
                layout: None,
            },
            crate::agent::HopCounter::new(0),
        );

        // Only 2 exchanges — fewer than the 3-exchange retention window
        for i in 0..2 {
            agent.recent_messages.push(Message::user(format!("q{i}")));
            agent
                .recent_messages
                .push(Message::assistant(format!("a{i}"), None));
        }
        assert_eq!(agent.message_count(), 4);

        agent.rotate_messages_after_observation();

        assert_eq!(
            agent.message_count(),
            4,
            "all messages should be retained when fewer than 3 exchanges exist"
        );
        assert_eq!(agent.messages_since(0)[0].content, "q0");
        assert_eq!(agent.messages_since(0)[3].content, "a1");
    }

    #[test]
    fn restore_messages_loads_into_history() {
        let mut agent = Agent::new(
            Box::new(MockProvider::new(vec![])),
            ToolRegistry::new(),
            empty_mcp(),
            IdentityFiles::default(),
            AgentConfig {
                options: CompletionOptions::default(),
                tz: chrono_tz::UTC,
                layout: None,
            },
            crate::agent::HopCounter::new(0),
        );

        let messages = vec![
            Message::user("persisted question"),
            Message::assistant("persisted answer", None),
        ];
        agent.restore_messages(messages);

        assert_eq!(
            agent.message_count(),
            2,
            "restored messages should be counted"
        );
        assert_eq!(
            agent.messages_since(0)[0].content,
            "persisted question",
            "restored content should match"
        );
        assert_eq!(agent.messages_since(0)[1].content, "persisted answer");
    }

    /// Mock provider with a configurable model name for `swap_provider` tests.
    struct NamedMockProvider {
        name: &'static str,
    }

    #[async_trait]
    impl InferenceProvider for NamedMockProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _options: &CompletionOptions,
        ) -> Result<InferenceResponse, InferenceError> {
            Ok(InferenceResponse::new("ok".to_string(), vec![]))
        }

        fn model_name(&self) -> &'static str {
            self.name
        }
    }

    #[test]
    fn swap_provider_changes_model() {
        let mut agent = Agent::new(
            Box::new(NamedMockProvider { name: "model-a" }),
            ToolRegistry::new(),
            empty_mcp(),
            IdentityFiles::default(),
            AgentConfig {
                options: CompletionOptions::default(),
                tz: chrono_tz::UTC,
                layout: None,
            },
            crate::agent::HopCounter::new(0),
        );

        assert_eq!(agent.provider.model_name(), "model-a");

        agent.swap_provider(
            Box::new(NamedMockProvider { name: "model-b" }),
            CompletionOptions::default(),
        );

        assert_eq!(agent.provider.model_name(), "model-b");
    }

    #[test]
    fn swap_provider_preserves_message_history() {
        let mut agent = Agent::new(
            Box::new(NamedMockProvider { name: "model-a" }),
            ToolRegistry::new(),
            empty_mcp(),
            IdentityFiles::default(),
            AgentConfig {
                options: CompletionOptions::default(),
                tz: chrono_tz::UTC,
                layout: None,
            },
            crate::agent::HopCounter::new(0),
        );

        // Inject some messages into history
        agent.inject_system_message("system context".to_string());
        agent.inject_inbound_message(make_inbound("m1", "user question"));
        let before_count = agent.message_count();
        assert!(before_count >= 2, "should have at least 2 messages");

        // Swap the provider
        agent.swap_provider(
            Box::new(NamedMockProvider { name: "model-b" }),
            CompletionOptions::default(),
        );

        // History should be preserved
        assert_eq!(
            agent.message_count(),
            before_count,
            "message count should not change after swap"
        );
        assert_eq!(agent.provider.model_name(), "model-b");
    }

    /// Editing SOUL.md between turns must be reflected in the next turn's system
    /// prompt — the whole point of per-turn identity reloads (issue #103).
    #[tokio::test]
    async fn turn_picks_up_identity_edits_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let layout = crate::workspace::layout::WorkspaceLayout::new(dir.path());
        tokio::fs::write(layout.soul_md(), "SOUL_MARKER_V1")
            .await
            .unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::channel(32);
        let provider = CapturingProvider::new(
            vec![
                InferenceResponse::new("turn one".to_string(), vec![]),
                InferenceResponse::new("turn two".to_string(), vec![]),
            ],
            tx,
        );
        let captured = Arc::clone(&provider.captured);

        let mut agent = Agent::new(
            Box::new(provider),
            ToolRegistry::new(),
            empty_mcp(),
            IdentityFiles::default(),
            AgentConfig {
                options: CompletionOptions::default(),
                tz: chrono_tz::UTC,
                layout: Some(layout.clone()),
            },
            crate::agent::HopCounter::new(0),
        );

        let (publisher, ep) = test_bus();
        agent
            .process_message(
                "hi",
                &publisher,
                Some(&ep),
                None,
                "",
                None,
                &PromptContext::default(),
                &mut rx,
                &[],
                None,
                &CancellationToken::new(),
            )
            .await
            .unwrap();

        tokio::fs::write(layout.soul_md(), "SOUL_MARKER_V2")
            .await
            .unwrap();

        agent
            .process_message(
                "hi again",
                &publisher,
                Some(&ep),
                None,
                "",
                None,
                &PromptContext::default(),
                &mut rx,
                &[],
                None,
                &CancellationToken::new(),
            )
            .await
            .unwrap();

        let calls = captured.lock().await;
        assert_eq!(calls.len(), 2, "provider should be called once per turn");
        assert!(
            calls[0][0].content.contains("SOUL_MARKER_V1"),
            "turn 1 system prompt should contain the original soul"
        );
        assert!(
            calls[1][0].content.contains("SOUL_MARKER_V2"),
            "turn 2 system prompt should reflect the edited soul"
        );
        assert!(
            !calls[1][0].content.contains("SOUL_MARKER_V1"),
            "turn 2 must not carry the stale soul snapshot"
        );
    }

    /// Deleting BOOTSTRAP.md (as happens after the first conversation) must make
    /// it vanish from the next turn's prompt rather than lingering until restart.
    #[tokio::test]
    async fn bootstrap_disappears_after_deletion() {
        let dir = tempfile::tempdir().unwrap();
        let layout = crate::workspace::layout::WorkspaceLayout::new(dir.path());
        tokio::fs::write(layout.bootstrap_md(), "BOOTSTRAP_MARKER")
            .await
            .unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::channel(32);
        let provider = CapturingProvider::new(
            vec![
                InferenceResponse::new("turn one".to_string(), vec![]),
                InferenceResponse::new("turn two".to_string(), vec![]),
            ],
            tx,
        );
        let captured = Arc::clone(&provider.captured);

        let mut agent = Agent::new(
            Box::new(provider),
            ToolRegistry::new(),
            empty_mcp(),
            IdentityFiles::default(),
            AgentConfig {
                options: CompletionOptions::default(),
                tz: chrono_tz::UTC,
                layout: Some(layout.clone()),
            },
            crate::agent::HopCounter::new(0),
        );

        let (publisher, ep) = test_bus();
        agent
            .process_message(
                "hi",
                &publisher,
                Some(&ep),
                None,
                "",
                None,
                &PromptContext::default(),
                &mut rx,
                &[],
                None,
                &CancellationToken::new(),
            )
            .await
            .unwrap();

        tokio::fs::remove_file(layout.bootstrap_md()).await.unwrap();

        agent
            .process_message(
                "hi again",
                &publisher,
                Some(&ep),
                None,
                "",
                None,
                &PromptContext::default(),
                &mut rx,
                &[],
                None,
                &CancellationToken::new(),
            )
            .await
            .unwrap();

        let calls = captured.lock().await;
        assert!(
            calls[0][0].content.contains("BOOTSTRAP_MARKER"),
            "turn 1 system prompt should include bootstrap guidance"
        );
        assert!(
            !calls[1][0].content.contains("BOOTSTRAP_MARKER"),
            "turn 2 system prompt should drop bootstrap after deletion"
        );
    }

    /// A read failure during reload (here: SOUL.md is a directory, a
    /// deterministic non-NotFound error on Linux) must fall back to the
    /// last-known-good snapshot instead of failing the turn.
    #[tokio::test]
    async fn identity_read_failure_falls_back_to_cache() {
        let dir = tempfile::tempdir().unwrap();
        let layout = crate::workspace::layout::WorkspaceLayout::new(dir.path());
        // A directory named SOUL.md makes read_to_string return a non-NotFound
        // error, so IdentityFiles::load fails and the reload falls back.
        tokio::fs::create_dir(layout.soul_md()).await.unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::channel(32);
        let provider =
            CapturingProvider::new(vec![InferenceResponse::new("ok".to_string(), vec![])], tx);
        let captured = Arc::clone(&provider.captured);

        let cached = IdentityFiles {
            soul: Some("cached soul".to_string()),
            ..IdentityFiles::default()
        };

        let mut agent = Agent::new(
            Box::new(provider),
            ToolRegistry::new(),
            empty_mcp(),
            cached,
            AgentConfig {
                options: CompletionOptions::default(),
                tz: chrono_tz::UTC,
                layout: Some(layout.clone()),
            },
            crate::agent::HopCounter::new(0),
        );

        let (publisher, ep) = test_bus();
        let result = agent
            .process_message(
                "hi",
                &publisher,
                Some(&ep),
                None,
                "",
                None,
                &PromptContext::default(),
                &mut rx,
                &[],
                None,
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(result, vec!["ok"], "turn should succeed despite read error");

        let calls = captured.lock().await;
        assert!(
            calls[0][0].content.contains("cached soul"),
            "system prompt should fall back to the cached soul snapshot"
        );
    }

    fn test_agent(tools: ToolRegistry) -> Agent {
        Agent::new(
            Box::new(MockProvider::new(vec![])),
            tools,
            empty_mcp(),
            IdentityFiles::default(),
            AgentConfig {
                options: CompletionOptions::default(),
                tz: chrono_tz::UTC,
                layout: None,
            },
            crate::agent::HopCounter::new(0),
        )
    }

    #[test]
    fn reload_ollama_web_search_tool_adds_it_when_backend_is_ollama() {
        let mut agent = test_agent(ToolRegistry::new());
        assert!(
            !agent
                .tools
                .tool_names()
                .contains(&"ollama_web_search".to_string()),
            "should start without the tool"
        );

        let backend = crate::config::StandaloneBackendConfig {
            name: "ollama".to_string(),
            api_key: "key".to_string(),
            base_url: None,
        };
        agent.reload_ollama_web_search_tool(Some(&backend));

        assert!(
            agent
                .tools
                .tool_names()
                .contains(&"ollama_web_search".to_string()),
            "should register the tool once the backend names ollama"
        );
    }

    #[test]
    fn reload_ollama_web_search_tool_removes_it_when_backend_changes_away() {
        let mut agent = test_agent(ToolRegistry::new());
        let backend = crate::config::StandaloneBackendConfig {
            name: "ollama".to_string(),
            api_key: "key".to_string(),
            base_url: None,
        };
        agent.reload_ollama_web_search_tool(Some(&backend));
        assert!(
            agent
                .tools
                .tool_names()
                .contains(&"ollama_web_search".to_string())
        );

        agent.reload_ollama_web_search_tool(None);
        assert!(
            !agent
                .tools
                .tool_names()
                .contains(&"ollama_web_search".to_string()),
            "should remove the tool once no standalone backend is configured"
        );
    }

    #[test]
    fn reload_ollama_web_search_tool_leaves_other_tools_alone() {
        let mut registry = ToolRegistry::new();
        registry.register_defaults(FileTracker::new_shared(), PathPolicy::new_shared());
        let mut agent = test_agent(registry);

        let backend = crate::config::StandaloneBackendConfig {
            name: "ollama".to_string(),
            api_key: "key".to_string(),
            base_url: None,
        };
        agent.reload_ollama_web_search_tool(Some(&backend));
        agent.reload_ollama_web_search_tool(None);

        let names = agent.tools.tool_names();
        assert!(
            names.contains(&"exec".to_string()),
            "unrelated tools should survive reload"
        );
    }
}
