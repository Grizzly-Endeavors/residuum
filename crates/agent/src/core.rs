//! Agent struct, configuration, and turn dispatch.

use tokio_util::sync::CancellationToken;

use residuum_bus::Publisher;
use residuum_bus::origin::MessageOrigin;
use residuum_inference::{CompletionOptions, InferenceProvider, Message};
use residuum_mcp::SharedMcpRegistry;
use residuum_workspace::identity::IdentityFiles;

use super::context::{MemoryContext, PromptContext, StatusLine};
use super::hop::HopCounter;
use super::recent_messages::RecentMessages;
use super::tool::ToolRegistry;
use super::turn::{EventContext, EventTarget, TurnOutput, TurnResources, execute_turn};
use super::usage::MainUsageSink;
use residuum_bus::interrupt;
use residuum_bus::usage::SessionUsageTotals;

/// Configuration for creating a new `Agent`.
pub struct AgentConfig {
    pub options: CompletionOptions,
    pub tz: chrono_tz::Tz,
    /// Workspace layout used to reload identity files from disk each turn.
    ///
    /// `None` pins the agent to the identity snapshot passed at construction
    /// (used by tests that don't touch the filesystem).
    pub layout: Option<residuum_config::WorkspaceLayout>,
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
    /// Maximum tool-call iterations before a turn stops itself gracefully.
    /// `None` (the default) means unlimited — set from
    /// [`residuum_config::AgentAbilitiesConfig::max_tool_iterations`] at
    /// startup and kept current on every config reload (see
    /// [`Agent::set_max_tool_iterations`]).
    max_tool_iterations: Option<usize>,
    /// Guards against a model repeating the exact same tool call. Set from
    /// [`residuum_config::AgentAbilitiesConfig::repeat_call_guard`] at startup
    /// and kept current on every config reload (see
    /// [`Agent::set_repeat_call_guard`]).
    repeat_call_guard: residuum_config::RepeatCallGuardConfig,
    observations: Option<String>,
    /// Narrative summary from the most recent observation cycle.
    recent_context: Option<String>,
    tz: chrono_tz::Tz,
    /// Workspace layout for per-turn identity reloads. `None` pins identity to
    /// the construction-time snapshot (test constructors).
    layout: Option<residuum_config::WorkspaceLayout>,
    /// The team roster read for the `TEAM` block at each turn entry. `None`
    /// leaves the prompt without a roster.
    team: Option<std::sync::Arc<dyn super::TeamRoster>>,
    /// Told as each endpoint reply of a turn reaches the main conversation,
    /// so the hub counts it read or unread at that moment. `None` counts
    /// nothing.
    reply_audience: Option<std::sync::Arc<dyn super::ReplyAudience>>,
    last_user_message_at: Option<chrono::NaiveDateTime>,
    /// Main's current-turn hop count: the highest hop count among the
    /// inputs (kickoff message, agent-message interrupts drained mid-turn)
    /// driving whichever turn is currently running. Shared with the
    /// `message_agent`/`subagent_spawn` tools registered against this agent,
    /// so they can compute the hop count an outgoing message or spawn
    /// carries without a separate side channel.
    hop_counter: HopCounter,
    /// Cumulative token usage totals for the main chat, shared with every
    /// turn's [`MainUsageSink`] so they all accumulate onto the same
    /// running total. Restored from disk at startup (see
    /// [`Self::restore_usage_totals`]); never read by the agent itself —
    /// see `docs/systems-usage/turn-control.md`.
    usage_totals: std::sync::Arc<tokio::sync::Mutex<SessionUsageTotals>>,
    /// Where `usage_totals` is persisted after every model call. `None`
    /// for an agent with no workspace layout (test constructors) — totals
    /// then live only in memory for the process's lifetime.
    usage_totals_path: Option<std::path::PathBuf>,
    /// The latest turn, while it is the latest and it failed. Set when a
    /// turn fails; cleared when another turn starts, when a rerun of it
    /// starts, and when history is cleared or rotated (the message the
    /// record points at may be gone).
    failed_turn: Option<FailedTurn>,
}

/// What a main-agent turn runs against besides the agent itself: where it
/// publishes, the prompt context, and what interrupts and stops it.
pub struct TurnWiring<'a> {
    /// Publishes the turn's events.
    pub publisher: &'a Publisher,
    /// Where the turn's output goes.
    pub output: TurnOutput<'a>,
    /// The skills context for the system prompt.
    pub prompt_ctx: &'a PromptContext<'a>,
    /// Messages, findings and stops that reach the turn while it runs.
    pub interrupt_rx: &'a mut dyn interrupt::InterruptSource,
    /// The subconscious watching the turn, when one does.
    pub subconscious: Option<&'a crate::subconscious::SubconsciousWatch>,
    /// Cancels the turn's in-flight model call.
    pub stop_token: &'a CancellationToken,
}

/// The agent's latest turn, which failed. The message that started it is
/// still in history, so [`Agent::rerun_turn`] can run the turn again on it.
/// Only a turn with an origin is recorded: a rerun runs under it.
#[derive(Debug)]
pub struct FailedTurn {
    /// The turn's correlation id.
    correlation_id: String,
    /// Index in the recent history of the message that started the turn.
    kickoff_index: usize,
    /// Where the message that started the turn came from.
    origin: MessageOrigin,
    /// The hop count that message carried.
    kickoff_hop: u32,
    /// When the message before it arrived, for the rerun's status line.
    previous_message_at: Option<chrono::NaiveDateTime>,
}

impl FailedTurn {
    /// The failed turn's correlation id.
    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn correlation_id(&self) -> &str {
        &self.correlation_id
    }

    /// Where the message that started the turn came from.
    #[must_use]
    pub fn origin(&self) -> &MessageOrigin {
        &self.origin
    }

    /// Index in the agent's recent history of the message that started the
    /// turn: the turn's messages are the ones from here on.
    #[must_use]
    pub fn kickoff_index(&self) -> usize {
        self.kickoff_index
    }
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
        let usage_totals_path = config
            .layout
            .as_ref()
            .map(residuum_config::WorkspaceLayout::usage_totals_json);
        Self {
            provider,
            tools,
            mcp_registry,
            identity,
            recent_messages: RecentMessages::new(),
            options: config.options,
            max_tool_iterations: None,
            repeat_call_guard: residuum_config::RepeatCallGuardConfig::default(),
            observations: None,
            recent_context: None,
            tz: config.tz,
            layout: config.layout,
            team: None,
            reply_audience: None,
            last_user_message_at: None,
            hop_counter,
            usage_totals: std::sync::Arc::new(tokio::sync::Mutex::new(
                SessionUsageTotals::default(),
            )),
            usage_totals_path,
            failed_turn: None,
        }
    }

    /// Restore persisted usage totals at startup, so the conversation size shows
    /// correct totals across a restart instead of resetting to zero.
    pub async fn restore_usage_totals(&mut self, totals: SessionUsageTotals) {
        *self.usage_totals.lock().await = totals;
    }

    /// Current cumulative usage totals.
    #[cfg(test)]
    #[must_use]
    pub(crate) async fn usage_totals_snapshot(&self) -> SessionUsageTotals {
        *self.usage_totals.lock().await
    }

    /// Load a fresh identity snapshot from disk. On read failure, logs an
    /// error and falls back to the last-known-good snapshot.
    async fn load_identity_snapshot(&self) -> IdentityFiles {
        let Some(layout) = &self.layout else {
            return self.identity.clone();
        };
        match IdentityFiles::load(layout).await {
            Ok(identity) => match &self.team {
                Some(team) => identity.with_team_roster(team.prompt_block()),
                None => identity,
            },
            Err(e) => {
                tracing::error!(error = %e, "identity reload failed; using last-known-good snapshot from previous turn");
                self.identity.clone()
            }
        }
    }

    /// Show this agent's teammates in its prompt: the `TEAM` block is rebuilt
    /// from `team` at every turn entry.
    pub fn set_team(&mut self, team: std::sync::Arc<dyn super::TeamRoster>) {
        self.team = Some(team);
    }

    /// Tell `audience` as each of a turn's endpoint replies reaches the main
    /// conversation.
    pub fn set_reply_audience(&mut self, audience: std::sync::Arc<dyn super::ReplyAudience>) {
        self.reply_audience = Some(audience);
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

    /// Apply a new user timezone to future turns' timestamps.
    pub fn set_timezone(&mut self, tz: chrono_tz::Tz) {
        self.tz = tz;
    }

    /// The agent's tool registry, for re-registering tools in place (e.g.
    /// after a config reload).
    pub fn tools_mut(&mut self) -> &mut ToolRegistry {
        &mut self.tools
    }

    /// Set the maximum tool-call iterations for future turns (e.g. at
    /// startup, or after a config reload). `None` means unlimited.
    pub fn set_max_tool_iterations(&mut self, limit: Option<usize>) {
        self.max_tool_iterations = limit;
    }

    /// Set the repeat-call guard config for future turns (e.g. at startup,
    /// or after a config reload).
    pub fn set_repeat_call_guard(&mut self, config: residuum_config::RepeatCallGuardConfig) {
        self.repeat_call_guard = config;
    }

    /// Reload observations from the observation log file.
    ///
    /// # Errors
    /// Returns an error if the file exists but cannot be read or parsed.
    pub async fn reload_observations(
        &mut self,
        layout: &residuum_config::WorkspaceLayout,
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
        layout: &residuum_config::WorkspaceLayout,
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
        self.forget_failed_turn("history cleared");
        self.recent_messages.clear();
    }

    /// Rotate messages after an observation cycle.
    ///
    /// Extracts the last 3 text exchanges, clears the buffer, then prepends
    /// the retained exchanges so the agent keeps conversational context.
    pub fn rotate_messages_after_observation(&mut self) {
        self.forget_failed_turn("history rotated after an observation");
        let retained = self.recent_messages.last_exchanges(3);
        self.recent_messages.clear();
        self.recent_messages.prepend(retained);
    }

    /// The latest turn, when it failed and can still be run again.
    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn failed_turn(&self) -> Option<&FailedTurn> {
        self.failed_turn.as_ref()
    }

    /// Take the failed latest turn to run again, when `correlation_id` names
    /// it. Taken, it is no longer there to retry: a rerun that fails records
    /// itself again.
    pub fn take_failed_turn(&mut self, correlation_id: &str) -> Option<FailedTurn> {
        self.failed_turn
            .take_if(|failed| failed.correlation_id == correlation_id)
    }

    /// Drop the failed-turn record: the conversation moved past it, or the
    /// history it points into changed.
    fn forget_failed_turn(&mut self, reason: &str) {
        if let Some(failed) = self.failed_turn.take() {
            tracing::debug!(
                correlation_id = %failed.correlation_id,
                reason,
                "failed turn can no longer be retried"
            );
        }
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
    pub fn inject_inbound_message(&mut self, message: residuum_bus::MessageEvent) {
        self.forget_failed_turn("a message entered the conversation");
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

    /// Process a user message through the model, executing tool calls as needed.
    ///
    /// Returns a vec containing the final text-only response. Intermediate texts
    /// emitted alongside tool calls are published to the bus as they are
    /// produced (the main conversation, and the topic of the chat interface
    /// `output_endpoint` names at that moment, if it is one) but not included in the return
    /// value. `correlation_id` names the turn in
    /// everything it publishes.
    ///
    /// A turn that fails is recorded as the failed latest turn, when it has
    /// an `origin` to run again under, and [`Self::rerun_turn`] can then run
    /// it again on the message already in history.
    ///
    /// # Errors
    /// Returns an error if the model call fails or tool execution errors
    /// are unrecoverable.
    #[expect(
        clippy::too_many_arguments,
        reason = "publisher and topic params added during bus migration"
    )]
    #[tracing::instrument(skip_all)]
    pub async fn process_message<'o>(
        &mut self,
        user_input: &str,
        publisher: &Publisher,
        output_endpoint: impl Into<TurnOutput<'o>>,
        correlation_id: &str,
        origin: Option<&MessageOrigin>,
        prompt_ctx: &PromptContext<'_>,
        interrupt_rx: &mut dyn interrupt::InterruptSource,
        images: &[residuum_inference::ImageData],
        subconscious: Option<&crate::subconscious::SubconsciousWatch>,
        stop_token: &CancellationToken,
    ) -> anyhow::Result<Vec<String>> {
        tracing::debug!("processing user message");
        self.forget_failed_turn("another turn started");
        let now = residuum_core::time::now_local(self.tz);
        let previous_message_at = self.last_user_message_at;
        let status_line = StatusLine {
            now,
            last_message_at: previous_message_at,
            message_source: origin.map(|o| o.endpoint.clone()),
        };
        self.last_user_message_at = Some(now);

        let sender = origin.and_then(|o| o.sender.clone());
        // A teammate's or session's message can start a turn (it may arrive
        // when no turn is running, or after the last checkpoint of one), so
        // the kickoff carries the structured sender the way a message
        // injected mid-turn does.
        let agent_sender = origin.and_then(|o| o.agent_sender.as_deref().cloned());
        let kickoff = if images.is_empty() {
            Message::user(user_input)
        } else {
            Message::user_with_images(user_input, images.to_vec())
        };
        let kickoff_index = self.recent_messages.len();
        self.recent_messages
            .push(kickoff.with_sender(sender).with_agent_sender(agent_sender));

        let kickoff_hop = self.hop_counter.get();
        let wiring = TurnWiring {
            publisher,
            output: output_endpoint.into(),
            prompt_ctx,
            interrupt_rx,
            subconscious,
            stop_token,
        };
        let result = self
            .run_turn_from(kickoff_index, correlation_id, &status_line, wiring)
            .await;
        if result.is_err()
            && let Some(origin) = origin
        {
            self.failed_turn = Some(FailedTurn {
                correlation_id: correlation_id.to_string(),
                kickoff_index,
                origin: origin.clone(),
                kickoff_hop,
                previous_message_at,
            });
        }
        result
    }

    /// Run the failed latest turn again, taken with
    /// [`Self::take_failed_turn`], on the message that started it, which is
    /// still in history: no message is added. The failed attempt's work stays
    /// in history and the rerun continues from it, under the same correlation
    /// id. A rerun that fails is recorded as the failed latest turn again.
    ///
    /// # Errors
    /// As [`Self::process_message`].
    #[tracing::instrument(skip_all, fields(correlation_id = %failed.correlation_id))]
    pub async fn rerun_turn(
        &mut self,
        failed: FailedTurn,
        wiring: TurnWiring<'_>,
    ) -> anyhow::Result<Vec<String>> {
        tracing::info!(
            kickoff_index = failed.kickoff_index,
            "running the failed latest turn again"
        );
        // The rerun's inputs are the original turn's: the hop count its
        // opening message carried, and the status line it was sent under.
        self.hop_counter.bump(failed.kickoff_hop);
        let status_line = StatusLine {
            now: residuum_core::time::now_local(self.tz),
            last_message_at: failed.previous_message_at,
            message_source: Some(failed.origin.endpoint.clone()),
        };
        let result = self
            .run_turn_from(
                failed.kickoff_index,
                &failed.correlation_id,
                &status_line,
                wiring,
            )
            .await;
        if result.is_err() {
            self.failed_turn = Some(failed);
        }
        result
    }

    /// Run a turn whose opening message sits at `kickoff_index` in history,
    /// under `correlation_id`.
    async fn run_turn_from(
        &mut self,
        kickoff_index: usize,
        correlation_id: &str,
        status_line: &StatusLine,
        wiring: TurnWiring<'_>,
    ) -> anyhow::Result<Vec<String>> {
        self.identity = self.load_identity_snapshot().await;
        let TurnWiring {
            publisher,
            output,
            prompt_ctx,
            interrupt_rx,
            subconscious,
            stop_token,
        } = wiring;
        let memory_ctx =
            Self::memory_ctx(self.observations.as_deref(), self.recent_context.as_deref());
        let usage_sink = MainUsageSink::new(
            std::sync::Arc::clone(&self.usage_totals),
            self.usage_totals_path.clone(),
        );
        let resources = TurnResources {
            provider: &*self.provider,
            tools: &self.tools,
            mcp_registry: &self.mcp_registry,
            identity: &self.identity,
            options: &self.options,
            max_tool_iterations: self.max_tool_iterations,
            repeat_call_guard: self.repeat_call_guard,
            stop_token,
            // The main agent persists its transcript separately
            // (`recent_messages.json`, written after the whole turn).
            transcript_sink: None,
            usage_sink: Some(&usage_sink),
            hop_counter: &self.hop_counter,
        };
        let events = EventContext {
            publisher,
            reply_audience: self.reply_audience.as_deref(),
            target: EventTarget::Endpoint {
                output_endpoint: output,
                correlation_id,
            },
            session_conversation: None,
        };
        execute_turn(
            kickoff_index,
            &resources,
            &memory_ctx,
            prompt_ctx,
            &mut self.recent_messages,
            &events,
            Some(status_line),
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

        let token_count = |def: &residuum_inference::ToolDefinition| {
            let param_str = def.parameters.to_string();
            residuum_memory::tokens::estimate_tokens(&def.name)
                + residuum_memory::tokens::estimate_tokens(&def.description)
                + residuum_memory::tokens::estimate_tokens(&param_str)
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
    use super::*;
    use crate::testing::{CountingTool, EchoTool};
    use async_trait::async_trait;
    use residuum_inference::{InferenceError, InferenceResponse, ToolCall, ToolDefinition};
    use residuum_mcp::McpRegistry;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn empty_mcp() -> SharedMcpRegistry {
        McpRegistry::new_shared()
    }

    /// A registry holding only the echo tool, for a turn that needs some
    /// tool to call.
    fn echo_registry() -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoTool));
        registry
    }

    /// Create a test publisher and endpoint for bus-based tests.
    fn test_bus() -> (Publisher, residuum_bus::EndpointName) {
        let handle = residuum_bus::spawn_broker();
        let publisher = handle.publisher();
        let ep = residuum_bus::EndpointName::from("test");
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
            crate::HopCounter::new(0),
        );

        let (publisher, ep) = test_bus();
        let mut irx = residuum_bus::testing::dead_interrupt_rx();
        let result = agent
            .process_message(
                "hi",
                &publisher,
                Some(&ep),
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
    async fn process_message_accumulates_usage_totals_across_turns() {
        let provider = MockProvider::new(vec![
            InferenceResponse {
                content: "first".to_string(),
                tool_calls: vec![],
                usage: Some(residuum_inference::Usage {
                    input_tokens: 100,
                    output_tokens: 20,
                    cache_creation_tokens: None,
                    cache_read_tokens: None,
                }),
                thinking: Vec::new(),
                stop_reason: None,
                images_omitted: None,
                thinking_dropped: 0,
            },
            InferenceResponse {
                content: "second".to_string(),
                tool_calls: vec![],
                usage: Some(residuum_inference::Usage {
                    input_tokens: 150,
                    output_tokens: 30,
                    cache_creation_tokens: None,
                    cache_read_tokens: None,
                }),
                thinking: Vec::new(),
                stop_reason: None,
                images_omitted: None,
                thinking_dropped: 0,
            },
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
            crate::HopCounter::new(0),
        );

        let (publisher, ep) = test_bus();

        for msg in ["hi", "again"] {
            let mut irx = residuum_bus::testing::dead_interrupt_rx();
            agent
                .process_message(
                    msg,
                    &publisher,
                    Some(&ep),
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
        }

        let totals = agent.usage_totals_snapshot().await;
        assert_eq!(
            totals.input_tokens, 250,
            "totals should accumulate across separate turns"
        );
        assert_eq!(totals.output_tokens, 50);
        assert_eq!(
            totals.context_tokens,
            Some(150),
            "context size should reflect only the latest call"
        );
    }

    #[tokio::test]
    async fn restore_usage_totals_seeds_the_shared_running_total() {
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
            crate::HopCounter::new(0),
        );

        let mut restored = SessionUsageTotals::default();
        restored.accumulate(Some(residuum_inference::Usage {
            input_tokens: 500,
            output_tokens: 90,
            cache_creation_tokens: None,
            cache_read_tokens: None,
        }));
        agent.restore_usage_totals(restored).await;

        assert_eq!(agent.usage_totals_snapshot().await, restored);
    }

    #[tokio::test]
    async fn tool_loop_then_text() {
        let registry = echo_registry();

        let provider = MockProvider::new(vec![
            InferenceResponse::new(
                String::new(),
                vec![ToolCall {
                    id: "call_1".to_string(),
                    name: "echo".to_string(),
                    arguments: serde_json::json!({}),
                    server: None,
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
            crate::HopCounter::new(0),
        );

        let (publisher, ep) = test_bus();
        let mut irx = residuum_bus::testing::dead_interrupt_rx();
        let result = agent
            .process_message(
                "run echo test",
                &publisher,
                Some(&ep),
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
        let registry = echo_registry();

        // First response has text alongside tool calls (intermediate), second is final.
        let provider = MockProvider::new(vec![
            InferenceResponse::new(
                "Let me check that for you...".to_string(),
                vec![ToolCall {
                    id: "call_1".to_string(),
                    name: "echo".to_string(),
                    arguments: serde_json::json!({}),
                    server: None,
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
            crate::HopCounter::new(0),
        );

        let (publisher, ep) = test_bus();
        let mut irx = residuum_bus::testing::dead_interrupt_rx();
        let result = agent
            .process_message(
                "what does echo test print?",
                &publisher,
                Some(&ep),
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
    async fn unlimited_by_default_allows_more_than_fifty_tool_iterations() {
        // Deliberately well past the old hardcoded 50-iteration cap, proving
        // an unconfigured agent no longer bails out at any fixed count.
        let extra_iterations = 60;
        // Arguments vary per call (the loop index) so this exercises only
        // the max_tool_iterations gate, not the unrelated repeat-call guard,
        // which would otherwise end the turn on an identical call streak.
        let mut responses: Vec<InferenceResponse> = (0..extra_iterations)
            .map(|i| {
                InferenceResponse::new(
                    String::new(),
                    vec![ToolCall {
                        id: format!("call_{i}"),
                        name: "echo".to_string(),
                        arguments: serde_json::json!({"loop": i}),
                        server: None,
                    }],
                )
            })
            .collect();
        responses.push(InferenceResponse::new("done".to_string(), vec![]));

        let registry = echo_registry();

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
            crate::HopCounter::new(0),
        );
        // max_tool_iterations left unset: unlimited by default.

        let (publisher, ep) = test_bus();
        let mut irx = residuum_bus::testing::dead_interrupt_rx();
        let result = agent
            .process_message(
                "loop past the old cap",
                &publisher,
                Some(&ep),
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
            vec!["done"],
            "should complete normally well past the old 50-iteration cap"
        );
    }

    #[tokio::test]
    async fn configured_tool_iteration_limit_stops_the_turn_gracefully() {
        let limit = 3;
        let responses: Vec<InferenceResponse> = (0..limit + 5)
            .map(|i| {
                InferenceResponse::new(
                    String::new(),
                    vec![ToolCall {
                        id: format!("call_{i}"),
                        name: "echo".to_string(),
                        arguments: serde_json::json!({"loop": 0}),
                        server: None,
                    }],
                )
            })
            .collect();

        let registry = echo_registry();

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
            crate::HopCounter::new(0),
        );
        agent.set_max_tool_iterations(Some(limit));

        let (publisher, ep) = test_bus();
        let mut irx = residuum_bus::testing::dead_interrupt_rx();
        let result = agent
            .process_message(
                "loop forever",
                &publisher,
                Some(&ep),
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
            result.len(),
            1,
            "should end gracefully with one notice, not an error"
        );
        assert!(
            result[0].contains("stopped after 3 tool calls"),
            "message should mention the configured limit: {}",
            result[0]
        );
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
            crate::HopCounter::new(0),
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
            residuum_inference::Role::User,
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
            crate::HopCounter::new(0),
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
            residuum_inference::Role::System,
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
            crate::HopCounter::new(0),
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
            crate::HopCounter::new(0),
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
            crate::HopCounter::new(0),
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
        interrupt_tx: tokio::sync::mpsc::UnboundedSender<interrupt::Interrupt>,
    }

    impl CapturingProvider {
        fn new(
            responses: Vec<InferenceResponse>,
            interrupt_tx: tokio::sync::mpsc::UnboundedSender<interrupt::Interrupt>,
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
                drop(self.interrupt_tx.send(intr));
            }

            Ok(response)
        }

        fn model_name(&self) -> &'static str {
            "capturing-mock"
        }
    }

    fn make_inbound(id: &str, content: &str) -> residuum_bus::MessageEvent {
        residuum_bus::MessageEvent {
            id: id.to_string(),
            content: content.to_string(),
            origin: residuum_bus::origin::MessageOrigin {
                endpoint: "test".to_string(),
                sender: None,
                conversation: None,
                agent_sender: None,
            },
            timestamp: chrono::Utc::now().naive_utc(),
            images: vec![],
            attachments: Vec::new(),
            context: None,
        }
    }

    #[tokio::test]
    async fn interrupt_injects_user_message_mid_turn() {
        let registry = echo_registry();

        let (interrupt_tx, mut interrupt_rx) = tokio::sync::mpsc::unbounded_channel();

        let provider = CapturingProvider::new(
            vec![
                // Call 0: tool call — triggers tool loop iteration
                InferenceResponse::new(
                    String::new(),
                    vec![ToolCall {
                        id: "call_1".to_string(),
                        name: "echo".to_string(),
                        arguments: serde_json::json!({}),
                        server: None,
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
            crate::HopCounter::new(0),
        );

        let (publisher, ep) = test_bus();
        let result = agent
            .process_message(
                "hello",
                &publisher,
                Some(&ep),
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
        let registry = echo_registry();

        let (interrupt_tx, mut interrupt_rx) = tokio::sync::mpsc::unbounded_channel();

        let provider = CapturingProvider::new(
            vec![
                InferenceResponse::new(
                    String::new(),
                    vec![ToolCall {
                        id: "call_1".to_string(),
                        name: "echo".to_string(),
                        arguments: serde_json::json!({}),
                        server: None,
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
            crate::HopCounter::new(0),
        );

        let (publisher, ep) = test_bus();
        agent
            .process_message(
                "hello",
                &publisher,
                Some(&ep),
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
        let (interrupt_tx, mut interrupt_rx) = tokio::sync::mpsc::unbounded_channel();

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
            crate::HopCounter::new(0),
        );

        let (publisher, ep) = test_bus();
        let result = agent
            .process_message(
                "hello",
                &publisher,
                Some(&ep),
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
            crate::HopCounter::new(0),
        );

        let (publisher, ep) = test_bus();
        let mut irx = residuum_bus::testing::dead_interrupt_rx();
        let stop_token = CancellationToken::new();
        stop_token.cancel();

        let result = agent
            .process_message(
                "hello",
                &publisher,
                Some(&ep),
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
        // An Interrupt::Stopped that arrives while the turn is working lets
        // the queued tool call run to completion, and stops the loop at its
        // next checkpoint.
        let tool_runs = Arc::new(AtomicUsize::new(0));
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(CountingTool::new(Arc::clone(&tool_runs))));

        let (interrupt_tx, mut interrupt_rx) = tokio::sync::mpsc::unbounded_channel();
        let provider = CapturingProvider::new(
            vec![
                InferenceResponse::new(
                    String::new(),
                    vec![ToolCall {
                        id: "call_1".to_string(),
                        name: "counting_tool".to_string(),
                        arguments: serde_json::json!({}),
                        server: None,
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
            crate::HopCounter::new(0),
        );

        let (publisher, ep) = test_bus();
        let result = agent
            .process_message(
                "hello",
                &publisher,
                Some(&ep),
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
        assert_eq!(
            tool_runs.load(Ordering::SeqCst),
            1,
            "the tool queued with the stop should run exactly once"
        );
        let messages = agent.messages_since(0);
        assert!(
            messages
                .iter()
                .any(|m| m.role == residuum_inference::Role::Tool),
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
            crate::HopCounter::new(0),
        );

        let (publisher, ep) = test_bus();
        let mut irx = residuum_bus::testing::dead_interrupt_rx();
        let result = agent
            .process_message(
                "hello",
                &publisher,
                Some(&ep),
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
            crate::HopCounter::new(0),
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
            crate::HopCounter::new(0),
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
            crate::HopCounter::new(0),
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
            crate::HopCounter::new(0),
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
        let layout = residuum_config::WorkspaceLayout::new(dir.path());
        tokio::fs::write(layout.soul_md(), "SOUL_MARKER_V1")
            .await
            .unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
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
            crate::HopCounter::new(0),
        );

        let (publisher, ep) = test_bus();
        agent
            .process_message(
                "hi",
                &publisher,
                Some(&ep),
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
        let layout = residuum_config::WorkspaceLayout::new(dir.path());
        tokio::fs::write(layout.bootstrap_md(), "BOOTSTRAP_MARKER")
            .await
            .unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
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
            crate::HopCounter::new(0),
        );

        let (publisher, ep) = test_bus();
        agent
            .process_message(
                "hi",
                &publisher,
                Some(&ep),
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
        let layout = residuum_config::WorkspaceLayout::new(dir.path());
        // A directory named SOUL.md makes read_to_string return a non-NotFound
        // error, so IdentityFiles::load fails and the reload falls back.
        tokio::fs::create_dir(layout.soul_md()).await.unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
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
            crate::HopCounter::new(0),
        );

        let (publisher, ep) = test_bus();
        let result = agent
            .process_message(
                "hi",
                &publisher,
                Some(&ep),
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

    /// A step of a [`ScriptedProvider`]'s script.
    type Step = Result<InferenceResponse, InferenceError>;

    /// A provider that answers each call with the next step of its script,
    /// and records the messages every call carried.
    struct ScriptedProvider {
        script: std::sync::Mutex<std::collections::VecDeque<Step>>,
        requests: Arc<std::sync::Mutex<Vec<Vec<Message>>>>,
    }

    #[async_trait]
    impl InferenceProvider for ScriptedProvider {
        async fn complete(
            &self,
            messages: &[Message],
            _tools: &[ToolDefinition],
            _options: &CompletionOptions,
        ) -> Result<InferenceResponse, InferenceError> {
            self.requests.lock().unwrap().push(messages.to_vec());
            self.script
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Err(InferenceError::Api("the script ran out".to_string())))
        }

        fn model_name(&self) -> &'static str {
            "scripted"
        }
    }

    fn model_down() -> Step {
        Err(InferenceError::Api("the model is down".to_string()))
    }

    fn reply(text: &str) -> InferenceResponse {
        InferenceResponse::new(text.to_string(), vec![])
    }

    fn echo_call() -> InferenceResponse {
        InferenceResponse::new(
            String::new(),
            vec![ToolCall {
                id: "call_1".to_string(),
                name: "echo".to_string(),
                arguments: serde_json::json!({}),
                server: None,
            }],
        )
    }

    /// An agent with the echo tool whose model follows `script`, and the
    /// messages each of its model calls carried.
    fn scripted_agent(script: Vec<Step>) -> (Agent, Arc<std::sync::Mutex<Vec<Vec<Message>>>>) {
        let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
        let provider = ScriptedProvider {
            script: std::sync::Mutex::new(script.into()),
            requests: Arc::clone(&requests),
        };
        let agent = Agent::new(
            Box::new(provider),
            echo_registry(),
            empty_mcp(),
            IdentityFiles::default(),
            AgentConfig {
                options: CompletionOptions::default(),
                tz: chrono_tz::UTC,
                layout: None,
            },
            crate::HopCounter::new(0),
        );
        (agent, requests)
    }

    /// A person's message from the web.
    fn web_origin() -> MessageOrigin {
        MessageOrigin {
            endpoint: "ws".to_string(),
            sender: None,
            conversation: None,
            agent_sender: None,
        }
    }

    async fn send(
        agent: &mut Agent,
        text: &str,
        correlation_id: &str,
        images: &[residuum_inference::ImageData],
    ) -> anyhow::Result<Vec<String>> {
        let (publisher, ep) = test_bus();
        let mut irx = residuum_bus::testing::dead_interrupt_rx();
        agent
            .process_message(
                text,
                &publisher,
                Some(&ep),
                correlation_id,
                Some(&web_origin()),
                &PromptContext::default(),
                &mut irx,
                images,
                None,
                &CancellationToken::new(),
            )
            .await
    }

    /// Take the failed latest turn `correlation_id` names and run it again.
    async fn retry(agent: &mut Agent, correlation_id: &str) -> anyhow::Result<Vec<String>> {
        let failed = agent
            .take_failed_turn(correlation_id)
            .expect("the failed latest turn is there to retry");
        let (publisher, ep) = test_bus();
        let mut irx = residuum_bus::testing::dead_interrupt_rx();
        let prompt_ctx = PromptContext::default();
        let stop = CancellationToken::new();
        agent
            .rerun_turn(
                failed,
                TurnWiring {
                    publisher: &publisher,
                    output: Some(&ep).into(),
                    prompt_ctx: &prompt_ctx,
                    interrupt_rx: &mut irx,
                    subconscious: None,
                    stop_token: &stop,
                },
            )
            .await
    }

    /// The roles of a request's messages after the system prompt.
    fn conversation_roles(request: &[Message]) -> Vec<residuum_inference::Role> {
        request
            .iter()
            .map(|m| m.role)
            .filter(|role| *role != residuum_inference::Role::System)
            .collect()
    }

    fn user_messages(agent: &Agent) -> usize {
        agent
            .messages_since(0)
            .iter()
            .filter(|m| m.role == residuum_inference::Role::User)
            .count()
    }

    #[tokio::test]
    async fn a_failed_turn_runs_again_on_its_message_and_keeps_its_partial_work() {
        use residuum_inference::Role;

        let (mut agent, requests) =
            scripted_agent(vec![Ok(echo_call()), model_down(), Ok(reply("done"))]);
        agent.hop_counter().set(3);
        let failed = send(&mut agent, "check the logs", "t1", &[]).await;
        assert!(failed.is_err(), "the turn's second model call failed");
        let recorded = agent.failed_turn().expect("the failure is recorded");
        assert_eq!(recorded.correlation_id(), "t1");
        assert_eq!(recorded.kickoff_index(), 0);
        // The event loop resets the hop counter after every turn.
        agent.hop_counter().set(0);

        let replies = retry(&mut agent, "t1").await.unwrap();

        assert_eq!(replies, vec!["done"]);
        let rerun_request = requests.lock().unwrap().last().cloned().unwrap();
        assert_eq!(
            conversation_roles(&rerun_request),
            [Role::User, Role::Assistant, Role::Tool],
            "the rerun sends the recorded message and the failed attempt's tool call and result"
        );
        assert_eq!(
            rerun_request
                .iter()
                .find(|m| m.role == Role::User)
                .map(|m| m.content.as_str()),
            Some("check the logs")
        );
        assert_eq!(user_messages(&agent), 1, "no message is added by the rerun");
        let roles: Vec<_> = agent.messages_since(0).iter().map(|m| m.role).collect();
        assert_eq!(
            roles,
            [Role::User, Role::Assistant, Role::Tool, Role::Assistant],
            "the partial work stays and the reply follows it"
        );
        assert_eq!(
            agent.hop_counter().get(),
            3,
            "the rerun carries the hop count of the message that started the turn"
        );
        assert!(
            agent.failed_turn().is_none(),
            "a rerun that succeeds leaves nothing to retry"
        );
        assert!(
            agent.take_failed_turn("t1").is_none(),
            "a turn already retried can't be taken again"
        );
    }

    #[tokio::test]
    async fn a_failed_turn_is_only_retryable_by_its_id_and_until_another_turn_starts() {
        let (mut agent, _requests) = scripted_agent(vec![model_down(), Ok(reply("hi"))]);
        assert!(send(&mut agent, "first", "t1", &[]).await.is_err());

        assert!(agent.take_failed_turn("unknown").is_none());
        assert_eq!(
            agent.failed_turn().map(FailedTurn::correlation_id),
            Some("t1"),
            "naming another turn leaves the failed one in place"
        );

        send(&mut agent, "second", "t2", &[]).await.unwrap();
        assert!(
            agent.take_failed_turn("t1").is_none(),
            "another turn ran since, so the failed one is no longer the latest"
        );
    }

    #[tokio::test]
    async fn a_rerun_that_fails_is_recorded_again_and_can_be_retried() {
        let (mut agent, _requests) =
            scripted_agent(vec![model_down(), model_down(), Ok(reply("ok"))]);
        assert!(send(&mut agent, "hello", "t1", &[]).await.is_err());

        assert!(retry(&mut agent, "t1").await.is_err());
        let recorded = agent.failed_turn().expect("the failed rerun is recorded");
        assert_eq!(recorded.correlation_id(), "t1");
        assert_eq!(recorded.kickoff_index(), 0);

        assert_eq!(retry(&mut agent, "t1").await.unwrap(), vec!["ok"]);
        assert_eq!(user_messages(&agent), 1);
    }

    #[tokio::test]
    async fn a_failed_turn_without_an_origin_is_not_recorded() {
        let (mut agent, _requests) = scripted_agent(vec![model_down()]);
        let (publisher, ep) = test_bus();
        let mut irx = residuum_bus::testing::dead_interrupt_rx();

        let result = agent
            .process_message(
                "hello",
                &publisher,
                Some(&ep),
                "t1",
                None,
                &PromptContext::default(),
                &mut irx,
                &[],
                None,
                &CancellationToken::new(),
            )
            .await;

        assert!(result.is_err());
        assert!(
            agent.failed_turn().is_none(),
            "a rerun runs under the turn's origin, so one without it can't be retried"
        );
    }

    #[tokio::test]
    async fn rewriting_history_forgets_the_failed_turn() {
        let (mut agent, _requests) = scripted_agent(vec![model_down(), model_down()]);

        assert!(send(&mut agent, "one", "t1", &[]).await.is_err());
        agent.rotate_messages_after_observation();
        assert!(
            agent.failed_turn().is_none(),
            "rotation may drop the message the failed turn started from"
        );

        assert!(send(&mut agent, "two", "t2", &[]).await.is_err());
        agent.clear_messages();
        assert!(agent.failed_turn().is_none(), "clearing drops it");
    }

    #[tokio::test]
    async fn a_rerun_sends_the_attachments_already_in_history() {
        let image = residuum_inference::ImageData {
            media_type: "image/png".to_string(),
            data: "aGVsbG8=".to_string(),
        };
        let content = "what is in these?\n\n[Attached file: notes.txt]\nthe notes";
        let (mut agent, requests) = scripted_agent(vec![model_down(), Ok(reply("a pelican"))]);
        assert!(
            send(&mut agent, content, "t1", std::slice::from_ref(&image))
                .await
                .is_err()
        );

        retry(&mut agent, "t1").await.unwrap();

        let rerun_request = requests.lock().unwrap().last().cloned().unwrap();
        let users: Vec<&Message> = rerun_request
            .iter()
            .filter(|m| m.role == residuum_inference::Role::User)
            .collect();
        let [opening] = users.as_slice() else {
            panic!("the rerun sends the one recorded message: {users:?}");
        };
        assert_eq!(opening.content, content, "with the attached file's text");
        let images: Vec<(&str, &str)> = opening
            .images
            .iter()
            .map(|i| (i.media_type.as_str(), i.data.as_str()))
            .collect();
        assert_eq!(
            images,
            [(image.media_type.as_str(), image.data.as_str())],
            "and its image"
        );
    }
}
