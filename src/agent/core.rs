//! Agent struct, configuration, and turn dispatch.

use crate::bus::{EndpointName, Publisher};
use crate::interfaces::types::MessageOrigin;
use crate::mcp::SharedMcpRegistry;
use crate::models::{CompletionOptions, Message, ModelProvider};
use crate::tools::{SharedToolFilter, ToolRegistry};
use crate::workspace::identity::IdentityFiles;

use super::context::{MemoryContext, PromptContext, StatusLine};
use super::interrupt;
use super::recent_messages::RecentMessages;
use super::turn::{EventContext, TurnResources, execute_turn};

/// Result of a background system turn (pulse or scheduled action).
pub struct SystemTurnResult {
    /// The assistant's final text response.
    pub response: String,
    /// All messages from the background thread (user prompt + assistant response + tool calls).
    ///
    /// Feed these into `run_memory_pipeline()` so background turns contribute to memory.
    pub messages: Vec<Message>,
}

/// Configuration for creating a new `Agent`.
pub struct AgentConfig {
    pub options: CompletionOptions,
    pub tz: chrono_tz::Tz,
    pub inbox_dir: std::path::PathBuf,
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
    tz: chrono_tz::Tz,
    last_user_message_at: Option<chrono::NaiveDateTime>,
    /// Path to the inbox directory (for computing unread count per turn).
    inbox_dir: std::path::PathBuf,
}

impl Agent {
    /// Create a new agent with the given components.
    #[must_use]
    pub fn new(
        provider: Box<dyn ModelProvider>,
        tools: ToolRegistry,
        tool_filter: SharedToolFilter,
        mcp_registry: SharedMcpRegistry,
        identity: IdentityFiles,
        config: AgentConfig,
    ) -> Self {
        Self {
            provider,
            tools,
            tool_filter,
            mcp_registry,
            identity,
            recent_messages: RecentMessages::new(),
            options: config.options,
            observations: None,
            recent_context: None,
            tz: config.tz,
            last_user_message_at: None,
            inbox_dir: config.inbox_dir,
        }
    }

    /// Get a reference to the MCP registry.
    #[must_use]
    pub fn mcp_registry(&self) -> &SharedMcpRegistry {
        &self.mcp_registry
    }

    /// Replace the model provider and completion options in-place (e.g. after a config reload).
    pub fn swap_provider(&mut self, provider: Box<dyn ModelProvider>, options: CompletionOptions) {
        tracing::info!(
            old_model = self.provider.model_name(),
            new_model = provider.model_name(),
            "swapping model provider"
        );
        self.provider = provider;
        self.options = options;
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

    /// Clear gated tool permissions (used during idle project deactivation).
    pub async fn clear_tool_filter(&self) {
        self.tool_filter.write().await.clear_enabled();
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

    /// Inject a user message directly into the conversation history.
    ///
    /// Used for user messages that arrived as interrupts during a turn's final
    /// LLM call and were drained after the turn completed. Ensures the message
    /// is visible in the next turn without being lost.
    pub fn inject_user_message(&mut self, content: impl Into<String>) {
        self.recent_messages.push(Message::user(content));
    }

    /// Run an autonomous wake turn triggered by background results.
    ///
    /// Unlike `process_message`, this does NOT update `last_user_message_at`.
    /// Pushes a user-role kickoff message (required by models that reject
    /// assistant prefill) so the agent reviews injected background results.
    ///
    /// # Errors
    /// Returns an error if the model call fails or tool execution errors
    /// are unrecoverable.
    #[tracing::instrument(skip_all, fields(operation = "wake_turn"))]
    pub async fn run_wake_turn(
        &mut self,
        publisher: &Publisher,
        output_endpoint: Option<&EndpointName>,
        tool_activity_endpoint: Option<&EndpointName>,
        prompt_ctx: &PromptContext<'_>,
        interrupt_rx: &mut tokio::sync::mpsc::Receiver<interrupt::Interrupt>,
    ) -> anyhow::Result<Vec<String>> {
        tracing::debug!("processing wake turn");
        let now = crate::time::now_local(self.tz);
        let unread = crate::inbox::count_unread(&self.inbox_dir).await;
        let status_line = StatusLine {
            now,
            last_message_at: self.last_user_message_at,
            message_source: Some("background".to_string()),
            unread_inbox_count: unread,
        };

        // User-role kickoff — models require the conversation to end with a
        // user message. Tagged as background via the status line.
        self.recent_messages.push(Message::user(
            "[Background results require your attention. Review and take action.]",
        ));

        let memory_ctx = MemoryContext {
            observations: self.observations.as_deref(),
            recent_context: self.recent_context.as_deref(),
        };

        let resources = TurnResources {
            provider: &*self.provider,
            tools: &self.tools,
            tool_filter: &self.tool_filter,
            mcp_registry: &self.mcp_registry,
            identity: &self.identity,
            options: &self.options,
        };
        let events = EventContext {
            publisher,
            output_endpoint,
            tool_activity_endpoint,
            correlation_id: "",
        };
        execute_turn(
            &resources,
            &memory_ctx,
            prompt_ctx,
            &mut self.recent_messages,
            &events,
            Some(&status_line),
            interrupt_rx,
        )
        .await
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
        images: &[crate::models::ImageData],
    ) -> anyhow::Result<Vec<String>> {
        tracing::debug!("processing user message");
        let now = crate::time::now_local(self.tz);
        let unread = crate::inbox::count_unread(&self.inbox_dir).await;
        let status_line = StatusLine {
            now,
            last_message_at: self.last_user_message_at,
            message_source: origin.map(|o| o.endpoint.clone()),
            unread_inbox_count: unread,
        };
        self.last_user_message_at = Some(now);

        if images.is_empty() {
            self.recent_messages.push(Message::user(user_input));
        } else {
            self.recent_messages
                .push(Message::user_with_images(user_input, images.to_vec()));
        }

        let memory_ctx = MemoryContext {
            observations: self.observations.as_deref(),
            recent_context: self.recent_context.as_deref(),
        };

        let resources = TurnResources {
            provider: &*self.provider,
            tools: &self.tools,
            tool_filter: &self.tool_filter,
            mcp_registry: &self.mcp_registry,
            identity: &self.identity,
            options: &self.options,
        };
        let events = EventContext {
            publisher,
            output_endpoint,
            tool_activity_endpoint,
            correlation_id,
        };
        execute_turn(
            &resources,
            &memory_ctx,
            prompt_ctx,
            &mut self.recent_messages,
            &events,
            Some(&status_line),
            interrupt_rx,
        )
        .await
    }

    /// Run a background agent thread for pulse or scheduled action tasks.
    ///
    /// Creates a temporary message buffer that is not added to the main conversation.
    /// Returns both the response text and the thread messages so the caller
    /// can feed them into `run_memory_pipeline()`.
    ///
    /// If `provider_override` is `Some`, that provider is used instead of the
    /// agent's default provider for this turn only.
    ///
    /// # Errors
    /// Returns an error if the model call fails.
    #[tracing::instrument(skip_all, fields(operation = "system_turn"))]
    pub async fn run_system_turn(
        &self,
        prompt: &str,
        publisher: &Publisher,
        output_endpoint: Option<&EndpointName>,
        tool_activity_endpoint: Option<&EndpointName>,
        provider_override: Option<&dyn ModelProvider>,
        prompt_ctx: &PromptContext<'_>,
    ) -> anyhow::Result<SystemTurnResult> {
        let mut thread_messages = RecentMessages::new();
        thread_messages.push(Message::user(prompt));

        let provider: &dyn ModelProvider = provider_override.unwrap_or(&*self.provider);

        let memory_ctx = MemoryContext {
            observations: self.observations.as_deref(),
            recent_context: self.recent_context.as_deref(),
        };

        // System turns don't participate in interrupts — use a dead-end channel
        let mut sys_interrupt_rx = interrupt::dead_interrupt_rx();

        let resources = TurnResources {
            provider,
            tools: &self.tools,
            tool_filter: &self.tool_filter,
            mcp_registry: &self.mcp_registry,
            identity: &self.identity,
            options: &self.options,
        };

        let events = EventContext {
            publisher,
            output_endpoint,
            tool_activity_endpoint,
            correlation_id: "",
        };
        // System turns don't inject time context (no user-facing timestamps)
        let mut texts = execute_turn(
            &resources,
            &memory_ctx,
            prompt_ctx,
            &mut thread_messages,
            &events,
            None,
            &mut sys_interrupt_rx,
        )
        .await?;

        let response = texts
            .pop()
            .ok_or_else(|| anyhow::anyhow!("system turn returned no text responses"))?;

        Ok(SystemTurnResult {
            response,
            messages: thread_messages.messages().to_vec(),
        })
    }

    /// Compute a per-section token breakdown for the current agent context.
    pub async fn context_breakdown(
        &self,
        prompt_ctx: &PromptContext<'_>,
    ) -> super::context::ContextBreakdown {
        let memory_ctx = MemoryContext {
            observations: self.observations.as_deref(),
            recent_context: self.recent_context.as_deref(),
        };

        let filter = self.tool_filter.read().await;
        let builtin_defs = self.tools.definitions(&filter);
        drop(filter);

        let mcp_defs = self.mcp_registry.read().await.tool_definitions();

        let token_count = |def: &crate::models::ToolDefinition| {
            let param_str = def.parameters.to_string();
            crate::memory::tokens::estimate_tokens(&def.name)
                + crate::memory::tokens::estimate_tokens(&def.description)
                + crate::memory::tokens::estimate_tokens(&param_str)
        };
        let system_tool_tokens: usize = builtin_defs.iter().map(token_count).sum();
        let mcp_tool_tokens: usize = mcp_defs.iter().map(token_count).sum();

        super::context::compute_context_breakdown(
            &self.identity,
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
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "test code uses unwrap and indexing for clarity"
)]
mod tests {
    use super::super::turn::MAX_TOOL_ITERATIONS;
    use super::*;
    use crate::bus;
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
            AgentConfig {
                options: CompletionOptions::default(),
                tz: chrono_tz::UTC,
                inbox_dir: std::path::PathBuf::from("/tmp/residuum-test-inbox"),
            },
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
            )
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
            AgentConfig {
                options: CompletionOptions::default(),
                tz: chrono_tz::UTC,
                inbox_dir: std::path::PathBuf::from("/tmp/residuum-test-inbox"),
            },
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
            AgentConfig {
                options: CompletionOptions::default(),
                tz: chrono_tz::UTC,
                inbox_dir: std::path::PathBuf::from("/tmp/residuum-test-inbox"),
            },
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
            AgentConfig {
                options: CompletionOptions::default(),
                tz: chrono_tz::UTC,
                inbox_dir: std::path::PathBuf::from("/tmp/residuum-test-inbox"),
            },
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
            AgentConfig {
                options: CompletionOptions::default(),
                tz: chrono_tz::UTC,
                inbox_dir: std::path::PathBuf::from("/tmp/residuum-test-inbox"),
            },
        );

        let (publisher, ep) = test_bus();
        let result = agent
            .run_system_turn(
                "check status",
                &publisher,
                Some(&ep),
                None,
                None,
                &PromptContext::default(),
            )
            .await
            .unwrap();
        assert_eq!(result.response, "HEARTBEAT_OK", "response should match");
        assert_eq!(result.messages.len(), 2);
        assert_eq!(result.messages[0].role, crate::models::Role::User);
        assert_eq!(result.messages[0].content, "check status");
        assert_eq!(result.messages[1].content, "HEARTBEAT_OK");
        assert_eq!(
            agent.message_count(),
            0,
            "main message history should be untouched"
        );
    }

    #[test]
    fn inject_user_message_appears_in_history() {
        let mut agent = Agent::new(
            Box::new(MockProvider::new(vec![])),
            ToolRegistry::new(),
            no_filter(),
            empty_mcp(),
            IdentityFiles::default(),
            AgentConfig {
                options: CompletionOptions::default(),
                tz: chrono_tz::UTC,
                inbox_dir: std::path::PathBuf::from("/tmp/residuum-test-inbox"),
            },
        );

        agent.inject_user_message("leftover interrupt message");

        let msgs = agent.messages_since(0);
        assert_eq!(msgs.len(), 1, "should have one user message");
        assert_eq!(
            msgs[0].content, "leftover interrupt message",
            "user message content should match"
        );
        assert_eq!(
            msgs[0].role,
            crate::models::Role::User,
            "injected message should have User role"
        );
    }

    #[tokio::test]
    async fn wake_turn_pushes_user_kickoff_without_updating_timestamp() {
        let provider = MockProvider::new(vec![ModelResponse::new(
            "I'll handle it".to_string(),
            vec![],
        )]);

        let mut agent = Agent::new(
            Box::new(provider),
            ToolRegistry::new(),
            no_filter(),
            empty_mcp(),
            IdentityFiles::default(),
            AgentConfig {
                options: CompletionOptions::default(),
                tz: chrono_tz::UTC,
                inbox_dir: std::path::PathBuf::from("/tmp/residuum-test-inbox"),
            },
        );

        // Set a known timestamp so we can verify it doesn't change
        let fixed_time = chrono::NaiveDateTime::new(
            chrono::NaiveDate::from_ymd_opt(2025, 1, 1).unwrap(),
            chrono::NaiveTime::from_hms_opt(12, 0, 0).unwrap(),
        );
        agent.set_last_user_message_at(Some(fixed_time));

        // Inject a background result first (simulates what the gateway does)
        agent.inject_system_message("bg result: task completed");

        let (publisher, ep) = test_bus();
        let mut irx = interrupt::dead_interrupt_rx();
        let result = agent
            .run_wake_turn(
                &publisher,
                Some(&ep),
                None,
                &PromptContext::default(),
                &mut irx,
            )
            .await
            .unwrap();
        assert_eq!(result, vec!["I'll handle it"]);

        // Verify: last_user_message_at should be unchanged
        assert_eq!(
            agent.last_user_message_at,
            Some(fixed_time),
            "wake turn should not update last_user_message_at"
        );

        // Verify: kickoff message is present and uses user role (required by models)
        let msgs = agent.messages_since(0);
        let kickoff = msgs.iter().find(|m| {
            m.content
                .contains("Background results require your attention")
        });
        assert!(kickoff.is_some(), "wake turn should push kickoff message");
        assert_eq!(
            kickoff.unwrap().role,
            crate::models::Role::User,
            "kickoff must be user-role for model compatibility"
        );
    }

    #[test]
    fn inject_system_message_appears_in_history() {
        let mut agent = Agent::new(
            Box::new(MockProvider::new(vec![])),
            ToolRegistry::new(),
            no_filter(),
            empty_mcp(),
            IdentityFiles::default(),
            AgentConfig {
                options: CompletionOptions::default(),
                tz: chrono_tz::UTC,
                inbox_dir: std::path::PathBuf::from("/tmp/residuum-test-inbox"),
            },
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
            crate::models::Role::System,
            "injected message should have System role"
        );
    }

    #[test]
    fn injected_message_excluded_from_later_messages_since() {
        let mut agent = Agent::new(
            Box::new(MockProvider::new(vec![])),
            ToolRegistry::new(),
            no_filter(),
            empty_mcp(),
            IdentityFiles::default(),
            AgentConfig {
                options: CompletionOptions::default(),
                tz: chrono_tz::UTC,
                inbox_dir: std::path::PathBuf::from("/tmp/residuum-test-inbox"),
            },
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
            no_filter(),
            empty_mcp(),
            IdentityFiles::default(),
            AgentConfig {
                options: CompletionOptions::default(),
                tz: chrono_tz::UTC,
                inbox_dir: std::path::PathBuf::from("/tmp/residuum-test-inbox"),
            },
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
            no_filter(),
            empty_mcp(),
            IdentityFiles::default(),
            AgentConfig {
                options: CompletionOptions::default(),
                tz: chrono_tz::UTC,
                inbox_dir: std::path::PathBuf::from("/tmp/residuum-test-inbox"),
            },
        );
        agent.inject_user_message("hello");
        agent.inject_user_message("world");
        assert_eq!(agent.message_count(), 2);
        agent.clear_messages();
        assert_eq!(agent.message_count(), 0);
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

    fn make_inbound(id: &str, content: &str) -> crate::interfaces::types::InboundMessage {
        crate::interfaces::types::InboundMessage {
            id: id.to_string(),
            content: content.to_string(),
            origin: crate::interfaces::types::MessageOrigin {
                endpoint: "test".to_string(),
                sender_name: "tester".to_string(),
                sender_id: "t1".to_string(),
            },
            timestamp: chrono::Utc::now(),
            images: vec![],
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
            AgentConfig {
                options: CompletionOptions::default(),
                tz: chrono_tz::UTC,
                inbox_dir: std::path::PathBuf::from("/tmp/residuum-test-inbox"),
            },
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
            AgentConfig {
                options: CompletionOptions::default(),
                tz: chrono_tz::UTC,
                inbox_dir: std::path::PathBuf::from("/tmp/residuum-test-inbox"),
            },
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
            AgentConfig {
                options: CompletionOptions::default(),
                tz: chrono_tz::UTC,
                inbox_dir: std::path::PathBuf::from("/tmp/residuum-test-inbox"),
            },
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
        let provider = MockProvider::new(vec![
            ModelResponse::new(String::new(), vec![]),
            ModelResponse::new(String::new(), vec![]),
            ModelResponse::new(String::new(), vec![]),
        ]);

        let mut agent = Agent::new(
            Box::new(provider),
            ToolRegistry::new(),
            no_filter(),
            empty_mcp(),
            IdentityFiles::default(),
            AgentConfig {
                options: CompletionOptions::default(),
                tz: chrono_tz::UTC,
                inbox_dir: std::path::PathBuf::from("/tmp/residuum-test-inbox"),
            },
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
            no_filter(),
            empty_mcp(),
            IdentityFiles::default(),
            AgentConfig {
                options: CompletionOptions::default(),
                tz: chrono_tz::UTC,
                inbox_dir: std::path::PathBuf::from("/tmp/residuum-test-inbox"),
            },
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
            no_filter(),
            empty_mcp(),
            IdentityFiles::default(),
            AgentConfig {
                options: CompletionOptions::default(),
                tz: chrono_tz::UTC,
                inbox_dir: std::path::PathBuf::from("/tmp/residuum-test-inbox"),
            },
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
    impl ModelProvider for NamedMockProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _options: &CompletionOptions,
        ) -> Result<ModelResponse, ModelError> {
            Ok(ModelResponse::new("ok".to_string(), vec![]))
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
            no_filter(),
            empty_mcp(),
            IdentityFiles::default(),
            AgentConfig {
                options: CompletionOptions::default(),
                tz: chrono_tz::UTC,
                inbox_dir: std::path::PathBuf::from("/tmp/residuum-test-inbox"),
            },
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
            no_filter(),
            empty_mcp(),
            IdentityFiles::default(),
            AgentConfig {
                options: CompletionOptions::default(),
                tz: chrono_tz::UTC,
                inbox_dir: std::path::PathBuf::from("/tmp/residuum-test-inbox"),
            },
        );

        // Inject some messages into history
        agent.inject_system_message("system context".to_string());
        agent.inject_user_message("user question".to_string());
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
}
