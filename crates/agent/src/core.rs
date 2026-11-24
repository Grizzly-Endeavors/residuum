use tokio_util::sync::CancellationToken;

use residuum_bus::Publisher;
use residuum_bus::origin::MessageOrigin;
use residuum_inference::{CacheRetention, CompletionOptions, InferenceProvider, Message};
use residuum_mcp::SharedMcpRegistry;
use residuum_workspace::identity::IdentityFiles;

use super::context::{
    ContextBreakdown, ContextSnapshot, MemoryContext, PromptContext, StatusClock,
    compute_context_breakdown, save_context_snapshot,
};
use super::hop::HopCounter;
use super::recent_messages::RecentMessages;
use super::tool::ToolRegistry;
use super::turn::{
    EventContext, EventTarget, TurnOutput, TurnResources, build_model_request, execute_turn,
};
use super::usage::MainUsageSink;
use residuum_bus::interrupt;
use residuum_bus::usage::SessionUsageTotals;

pub struct AgentConfig {
    pub options: CompletionOptions,
    pub tz: chrono_tz::Tz,
    pub layout: Option<residuum_config::WorkspaceLayout>,
}

pub struct Agent {
    provider: Box<dyn InferenceProvider>,
    tools: ToolRegistry,
    mcp_registry: SharedMcpRegistry,
    identity: IdentityFiles,
    recent_messages: RecentMessages,
    options: CompletionOptions,
    max_tool_iterations: Option<usize>,
    repeat_call_guard: residuum_config::RepeatCallGuardConfig,
    observations: Option<String>,
    recent_context: Option<String>,
    status_clock: StatusClock,
    layout: Option<residuum_config::WorkspaceLayout>,
    team: Option<std::sync::Arc<dyn super::TeamRoster>>,
    reply_audience: Option<std::sync::Arc<dyn super::ReplyAudience>>,
    hop_counter: HopCounter,
    usage_totals: std::sync::Arc<tokio::sync::Mutex<SessionUsageTotals>>,
    failed_turn: Option<FailedTurn>,
}

pub struct TurnWiring<'a> {
    pub publisher: &'a Publisher,
    pub output: TurnOutput<'a>,
    pub prompt_ctx: &'a PromptContext<'a>,
    pub interrupt_rx: &'a mut dyn interrupt::InterruptSource,
    pub subconscious: Option<&'a crate::subconscious::SubconsciousWatch>,
    pub stop_token: &'a CancellationToken,
}

#[derive(Debug)]
pub struct FailedTurn {
    correlation_id: String,
    kickoff_index: usize,
    origin: MessageOrigin,
    kickoff_hop: u32,
}

impl FailedTurn {
    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn correlation_id(&self) -> &str {
        &self.correlation_id
    }

    #[must_use]
    pub fn origin(&self) -> &MessageOrigin {
        &self.origin
    }

    #[must_use]
    pub fn kickoff_index(&self) -> usize {
        self.kickoff_index
    }
}

impl Agent {
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
            max_tool_iterations: None,
            repeat_call_guard: residuum_config::RepeatCallGuardConfig::default(),
            observations: None,
            recent_context: None,
            status_clock: StatusClock::new(config.tz),
            layout: config.layout,
            team: None,
            reply_audience: None,
            hop_counter,
            usage_totals: std::sync::Arc::new(tokio::sync::Mutex::new(
                SessionUsageTotals::default(),
            )),
            failed_turn: None,
        }
    }

    pub async fn restore_usage_totals(&mut self, totals: SessionUsageTotals) {
        *self.usage_totals.lock().await = totals;
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) async fn usage_totals_snapshot(&self) -> SessionUsageTotals {
        *self.usage_totals.lock().await
    }

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

    pub fn set_team(&mut self, team: std::sync::Arc<dyn super::TeamRoster>) {
        self.team = Some(team);
    }

    pub fn set_reply_audience(&mut self, audience: std::sync::Arc<dyn super::ReplyAudience>) {
        self.reply_audience = Some(audience);
    }

    #[must_use]
    pub fn mcp_registry(&self) -> &SharedMcpRegistry {
        &self.mcp_registry
    }

    #[must_use]
    pub fn hop_counter(&self) -> &HopCounter {
        &self.hop_counter
    }

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

    pub fn set_timezone(&mut self, tz: chrono_tz::Tz) {
        self.status_clock.set_timezone(tz);
    }

    pub fn tools_mut(&mut self) -> &mut ToolRegistry {
        &mut self.tools
    }

    pub fn set_max_tool_iterations(&mut self, limit: Option<usize>) {
        self.max_tool_iterations = limit;
    }

    pub fn set_repeat_call_guard(&mut self, config: residuum_config::RepeatCallGuardConfig) {
        self.repeat_call_guard = config;
    }

    pub async fn reload_observations(
        &mut self,
        layout: &residuum_config::WorkspaceLayout,
    ) -> anyhow::Result<()> {
        self.observations =
            super::context::loading::load_observations(&layout.observations_json()).await?;
        Ok(())
    }

    pub async fn reload_recent_context(
        &mut self,
        layout: &residuum_config::WorkspaceLayout,
    ) -> anyhow::Result<()> {
        self.recent_context =
            super::context::loading::load_recent_context_narrative(&layout.recent_context_json())
                .await?;
        Ok(())
    }

    pub fn restore_messages(&mut self, messages: Vec<Message>) {
        self.recent_messages.extend(messages);
    }

    pub fn set_last_user_message_at(&mut self, at: Option<chrono::NaiveDateTime>) {
        self.status_clock.set_last_message_at(at);
    }

    pub fn clear_messages(&mut self) {
        self.forget_failed_turn("history cleared");
        self.recent_messages.clear();
    }

    pub fn rotate_messages_after_observation(&mut self) {
        self.forget_failed_turn("history rotated after an observation");
        let retained = self.recent_messages.last_exchanges(3);
        self.recent_messages.clear();
        self.recent_messages.prepend(retained);
    }

    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn failed_turn(&self) -> Option<&FailedTurn> {
        self.failed_turn.as_ref()
    }

    pub fn take_failed_turn(&mut self, correlation_id: &str) -> Option<FailedTurn> {
        self.failed_turn
            .take_if(|failed| failed.correlation_id == correlation_id)
    }

    fn forget_failed_turn(&mut self, reason: &str) {
        if let Some(failed) = self.failed_turn.take() {
            tracing::debug!(
                correlation_id = %failed.correlation_id,
                reason,
                "failed turn can no longer be retried"
            );
        }
    }

    pub fn inject_system_message(&mut self, content: impl Into<String>) {
        self.recent_messages.push(Message::system(content));
    }

    pub fn inject_inbound_message(&mut self, message: residuum_bus::MessageEvent) {
        self.forget_failed_turn("a message entered the conversation");
        let entries = self.status_clock.admit(message);
        self.recent_messages.extend(entries);
    }

    fn memory_ctx<'a>(
        observations: Option<&'a str>,
        recent_context: Option<&'a str>,
    ) -> MemoryContext<'a> {
        MemoryContext {
            observations,
            recent_context,
        }
    }

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

        let sender = origin.and_then(|o| o.sender.clone());
        let agent_sender = origin.and_then(|o| o.agent_sender.as_deref().cloned());
        let mut kickoff = if images.is_empty() {
            Message::user(user_input)
        } else {
            Message::user_with_images(user_input, images.to_vec())
        }
        .with_sender(sender)
        .with_agent_sender(agent_sender);
        self.status_clock
            .stamp(&mut kickoff, origin.map(|o| o.endpoint.as_str()));
        let kickoff_index = self.recent_messages.len();
        self.recent_messages.push(kickoff);

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
            .run_turn_from(
                kickoff_index,
                correlation_id,
                turn_cache_retention(origin),
                wiring,
            )
            .await;
        if result.is_err()
            && let Some(origin) = origin
        {
            self.failed_turn = Some(FailedTurn {
                correlation_id: correlation_id.to_string(),
                kickoff_index,
                origin: origin.clone(),
                kickoff_hop,
            });
        }
        result
    }

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
        self.hop_counter.bump(failed.kickoff_hop);
        let result = self
            .run_turn_from(
                failed.kickoff_index,
                &failed.correlation_id,
                turn_cache_retention(Some(&failed.origin)),
                wiring,
            )
            .await;
        if result.is_err() {
            self.failed_turn = Some(failed);
        }
        result
    }

    async fn run_turn_from(
        &mut self,
        kickoff_index: usize,
        correlation_id: &str,
        retention: CacheRetention,
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
            self.layout.as_ref(),
        );
        let mut options = self.options.clone();
        options.cache.retention = retention;
        let resources = TurnResources {
            provider: &*self.provider,
            tools: &self.tools,
            mcp_registry: &self.mcp_registry,
            identity: &self.identity,
            options: &options,
            max_tool_iterations: self.max_tool_iterations,
            repeat_call_guard: self.repeat_call_guard,
            stop_token,
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
        let result = execute_turn(
            kickoff_index,
            &resources,
            &memory_ctx,
            prompt_ctx,
            &mut self.recent_messages,
            &events,
            Some(&mut self.status_clock),
            interrupt_rx,
            subconscious,
        )
        .await;
        self.save_latest_breakdown(prompt_ctx).await;
        result
    }

    pub async fn context_breakdown(&self, prompt_ctx: &PromptContext<'_>) -> ContextBreakdown {
        let identity = self.load_identity_snapshot().await;
        self.breakdown_with(&identity, prompt_ctx).await
    }

    async fn breakdown_with(
        &self,
        identity: &IdentityFiles,
        prompt_ctx: &PromptContext<'_>,
    ) -> ContextBreakdown {
        let memory_ctx =
            Self::memory_ctx(self.observations.as_deref(), self.recent_context.as_deref());
        let request = build_model_request(
            identity,
            &self.recent_messages,
            &memory_ctx,
            prompt_ctx,
            &self.tools,
            &self.mcp_registry,
            &self.options,
        )
        .await;
        compute_context_breakdown(&request, &*self.provider)
    }

    pub async fn save_latest_breakdown(&self, prompt_ctx: &PromptContext<'_>) {
        let Some(layout) = &self.layout else {
            return;
        };
        let snapshot = ContextSnapshot {
            computed_at: chrono::Utc::now(),
            breakdown: self.breakdown_with(&self.identity, prompt_ctx).await,
        };
        save_context_snapshot(&layout.context_breakdown_json(), &snapshot).await;
    }

    #[must_use]
    pub fn message_count(&self) -> usize {
        self.recent_messages.len()
    }

    #[must_use]
    pub fn messages_since(&self, idx: usize) -> &[Message] {
        self.recent_messages.messages_since(idx)
    }
}

fn turn_cache_retention(origin: Option<&MessageOrigin>) -> CacheRetention {
    if origin.is_some_and(MessageOrigin::is_from_person) {
        CacheRetention::Long
    } else {
        CacheRetention::Short
    }
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "test code uses indexing for clarity"
)]
mod tests {
    use super::*;
    use crate::context::SkillsContext;
    use crate::testing::{CountingTool, EchoTool};
    use async_trait::async_trait;
    use residuum_inference::{InferenceError, InferenceResponse, ToolCall, ToolDefinition};
    use residuum_mcp::McpRegistry;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn empty_mcp() -> SharedMcpRegistry {
        McpRegistry::new_shared()
    }

    fn echo_registry() -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EchoTool));
        registry
    }

    fn test_bus() -> (Publisher, residuum_bus::EndpointName) {
        let handle = residuum_bus::spawn_broker();
        let publisher = handle.publisher();
        let ep = residuum_bus::EndpointName::from("test");
        (publisher, ep)
    }

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

    struct CacheHintsRecorder {
        seen: Arc<std::sync::Mutex<Vec<residuum_inference::CacheHints>>>,
    }

    #[async_trait]
    impl InferenceProvider for CacheHintsRecorder {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            options: &CompletionOptions,
        ) -> Result<InferenceResponse, InferenceError> {
            self.seen.lock().unwrap().push(options.cache.clone());
            Ok(InferenceResponse::new("ok".to_string(), vec![]))
        }

        fn model_name(&self) -> &'static str {
            "cache-hints-recorder"
        }
    }

    #[tokio::test]
    async fn a_person_s_turn_keeps_its_cache_longer_than_a_background_turn() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut options = CompletionOptions::default();
        options.cache.conversation_key = Some("main-conversation".to_string());
        let mut agent = Agent::new(
            Box::new(CacheHintsRecorder {
                seen: Arc::clone(&seen),
            }),
            ToolRegistry::new(),
            empty_mcp(),
            IdentityFiles::default(),
            AgentConfig {
                options,
                tz: chrono_tz::UTC,
                layout: None,
            },
            crate::HopCounter::new(0),
        );
        let background = MessageOrigin {
            endpoint: residuum_bus::origin::BACKGROUND_ENDPOINT.to_string(),
            ..web_origin()
        };

        for origin in [web_origin(), background] {
            let (publisher, ep) = test_bus();
            let mut irx = residuum_bus::testing::dead_interrupt_rx();
            agent
                .process_message(
                    "hi",
                    &publisher,
                    Some(&ep),
                    "",
                    Some(&origin),
                    &PromptContext::default(),
                    &mut irx,
                    &[],
                    None,
                    &CancellationToken::new(),
                )
                .await
                .unwrap();
        }

        let seen = seen.lock().unwrap().clone();
        let key = Some("main-conversation".to_string());
        assert_eq!(
            seen,
            vec![
                residuum_inference::CacheHints {
                    conversation_key: key.clone(),
                    retention: CacheRetention::Long,
                },
                residuum_inference::CacheHints {
                    conversation_key: key,
                    retention: CacheRetention::Short,
                },
            ],
            "the person's turn first, then the background one, both keyed to the conversation"
        );
    }

    #[test]
    fn only_a_person_s_message_asks_for_long_retention() {
        let from_agent = MessageOrigin {
            agent_sender: Some(Box::new(residuum_inference::AgentSender {
                address: "agent:other".to_string(),
                category: "teammate".to_string(),
                run_id: None,
            })),
            ..web_origin()
        };
        assert_eq!(
            turn_cache_retention(Some(&web_origin())),
            CacheRetention::Long
        );
        assert_eq!(
            turn_cache_retention(Some(&from_agent)),
            CacheRetention::Short
        );
        assert_eq!(turn_cache_retention(None), CacheRetention::Short);
    }

    #[tokio::test]
    async fn process_message_accumulates_usage_totals_across_turns() {
        let provider = MockProvider::new(vec![
            InferenceResponse {
                content: "first".to_string(),
                tool_calls: vec![],
                usage: Some(residuum_inference::Usage {
                    uncached_input_tokens: Some(100),
                    output_tokens: Some(20),
                    ..Default::default()
                }),
                thinking: Vec::new(),
                stop_reason: None,
                images_omitted: None,
                thinking_dropped: 0,
                attempts: Vec::new(),
            },
            InferenceResponse {
                content: "second".to_string(),
                tool_calls: vec![],
                usage: Some(residuum_inference::Usage {
                    uncached_input_tokens: Some(150),
                    output_tokens: Some(30),
                    ..Default::default()
                }),
                thinking: Vec::new(),
                stop_reason: None,
                images_omitted: None,
                thinking_dropped: 0,
                attempts: Vec::new(),
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
            uncached_input_tokens: Some(500),
            output_tokens: Some(90),
            ..Default::default()
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
        let extra_iterations = 60;
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

        agent.inject_system_message("bg result arrived");

        let before = agent.message_count();
        assert_eq!(before, 1, "injected message should be counted");

        agent.recent_messages.push(Message::user("hello"));
        agent
            .recent_messages
            .push(Message::assistant("hi there", None));

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

    struct CapturingProvider {
        responses: Vec<InferenceResponse>,
        call_count: Arc<AtomicUsize>,
        captured: Arc<tokio::sync::Mutex<Vec<Vec<Message>>>>,
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
            vec![InferenceResponse::new("final answer".to_string(), vec![])],
            interrupt_tx.clone(),
        );
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

        let pending = interrupt_rx.try_recv();
        assert!(
            pending.is_ok(),
            "interrupt should remain in the channel for the next turn"
        );
    }

    #[tokio::test]
    async fn stop_token_cancelled_before_call_aborts_without_invoking_provider() {
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

        agent.inject_system_message("system context".to_string());
        agent.inject_inbound_message(make_inbound("m1", "user question"));
        let before_count = agent.message_count();
        assert!(before_count >= 2, "should have at least 2 messages");

        agent.swap_provider(
            Box::new(NamedMockProvider { name: "model-b" }),
            CompletionOptions::default(),
        );

        assert_eq!(
            agent.message_count(),
            before_count,
            "message count should not change after swap"
        );
        assert_eq!(agent.provider.model_name(), "model-b");
    }

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

    type Step = Result<InferenceResponse, InferenceError>;

    #[derive(Clone)]
    struct RecordedRequest {
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
    }

    struct ScriptedProvider {
        script: std::sync::Mutex<std::collections::VecDeque<Step>>,
        requests: Arc<std::sync::Mutex<Vec<RecordedRequest>>>,
    }

    #[async_trait]
    impl InferenceProvider for ScriptedProvider {
        async fn complete(
            &self,
            messages: &[Message],
            tools: &[ToolDefinition],
            _options: &CompletionOptions,
        ) -> Result<InferenceResponse, InferenceError> {
            self.requests.lock().unwrap().push(RecordedRequest {
                messages: messages.to_vec(),
                tools: tools.to_vec(),
            });
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

    fn scripted_agent(script: Vec<Step>) -> (Agent, Arc<std::sync::Mutex<Vec<RecordedRequest>>>) {
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
    async fn each_model_call_is_sent_the_registry_tools() {
        let (mut agent, requests) = scripted_agent(vec![Ok(reply("hi"))]);
        send(&mut agent, "hello", "t1", &[]).await.unwrap();

        let recorded = requests.lock().unwrap();
        let names: Vec<&str> = recorded[0].tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["echo"]);
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
        agent.hop_counter().set(0);

        let replies = retry(&mut agent, "t1").await.unwrap();

        assert_eq!(replies, vec!["done"]);
        let rerun_request = requests.lock().unwrap().last().cloned().unwrap().messages;
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

        let rerun_request = requests.lock().unwrap().last().cloned().unwrap().messages;
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

    fn request_json(request: &[Message]) -> Vec<serde_json::Value> {
        request
            .iter()
            .map(|m| serde_json::to_value(m).unwrap())
            .collect()
    }

    fn assert_extends(earlier: &[Message], later: &[Message], what: &str) {
        let earlier = request_json(earlier);
        let later = request_json(later);
        assert!(
            later.len() > earlier.len(),
            "{what}: the later request appends ({} then {} messages)",
            earlier.len(),
            later.len()
        );
        assert_eq!(&later[..earlier.len()], earlier.as_slice(), "{what}");
    }

    fn is_status_line(message: &Message) -> bool {
        message.role == residuum_inference::Role::System
            && message.content.starts_with("[Received:")
    }

    async fn scripted_agent_with_workspace(
        script: Vec<Step>,
    ) -> (Agent, residuum_config::WorkspaceLayout, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let layout = residuum_config::WorkspaceLayout::new(dir.path());
        tokio::fs::create_dir_all(layout.memory_dir())
            .await
            .unwrap();
        let (mut agent, _) = scripted_agent(script);
        agent.layout = Some(layout.clone());
        (agent, layout, dir)
    }

    #[tokio::test]
    async fn each_turn_saves_the_breakdown_of_the_request_the_next_turn_starts_from() {
        use crate::context::load_context_snapshot;

        let (mut agent, layout, _dir) = scripted_agent_with_workspace(vec![
            Ok(echo_call()),
            Ok(reply("one")),
            Ok(reply("two")),
        ])
        .await;
        let path = layout.context_breakdown_json();
        assert_eq!(
            load_context_snapshot(&path).await.unwrap(),
            None,
            "nothing is saved before the first turn"
        );

        send(&mut agent, "first", "t1", &[]).await.unwrap();
        let first = load_context_snapshot(&path)
            .await
            .unwrap()
            .expect("the turn saved its breakdown");
        assert_eq!(
            first.breakdown,
            agent.context_breakdown(&PromptContext::default()).await,
            "the saved breakdown is the one /context shows"
        );
        assert_eq!(
            first.breakdown.history_count, 4,
            "the message, the tool call, its result and the reply"
        );
        assert!(
            first.breakdown.status_line_tokens > 0,
            "the message's status line is counted"
        );

        send(&mut agent, "second", "t2", &[]).await.unwrap();
        let second = load_context_snapshot(&path).await.unwrap().unwrap();
        assert_eq!(
            second.breakdown.history_count, 6,
            "replaced after the next turn"
        );
        assert!(second.breakdown.total_tokens > first.breakdown.total_tokens);
        assert!(second.computed_at >= first.computed_at);
    }

    #[tokio::test]
    async fn a_breakdown_that_cannot_be_saved_leaves_the_turn_alone() {
        let (mut agent, layout, _dir) = scripted_agent_with_workspace(vec![Ok(reply("one"))]).await;
        let path = layout.context_breakdown_json();
        tokio::fs::create_dir(&path).await.unwrap();

        let replies = send(&mut agent, "hi", "t1", &[]).await.unwrap();

        assert_eq!(replies, ["one"], "the turn answers as usual");
        assert!(
            tokio::fs::metadata(&path).await.unwrap().is_dir(),
            "nothing replaced what was in the way"
        );
    }

    #[tokio::test]
    async fn consecutive_turns_replay_the_earlier_request_unchanged() {
        use residuum_inference::Role;

        let (mut agent, requests) =
            scripted_agent(vec![Ok(echo_call()), Ok(reply("one")), Ok(reply("two"))]);
        send(&mut agent, "first", "t1", &[]).await.unwrap();
        send(&mut agent, "second", "t2", &[]).await.unwrap();

        let requests = requests.lock().unwrap().clone();
        assert_eq!(requests.len(), 3, "two calls in turn one, one in turn two");
        assert_extends(
            &requests[0].messages,
            &requests[1].messages,
            "the tool loop's second call extends its first",
        );
        assert_extends(
            &requests[1].messages,
            &requests[2].messages,
            "the next turn extends the previous turn's last request",
        );

        let tail: Vec<(Role, &str)> = requests[2].messages[requests[1].messages.len()..]
            .iter()
            .map(|m| (m.role, m.content.as_str()))
            .collect();
        let [reply_one, line, second] = tail.as_slice() else {
            panic!("expected the reply, the new line and the new message: {tail:?}");
        };
        assert_eq!(*reply_one, (Role::Assistant, "one"));
        assert_eq!(line.0, Role::System);
        assert!(
            line.1.starts_with("[Received:")
                && line.1.contains("[Last Message:")
                && line.1.ends_with("[Message Source: web]"),
            "the new message's line, computed at its arrival: {}",
            line.1
        );
        assert_eq!(*second, (Role::User, "second"));
        assert_eq!(
            requests[2]
                .messages
                .iter()
                .filter(|m| is_status_line(m))
                .count(),
            2,
            "one line per user message"
        );
    }

    #[tokio::test]
    async fn a_mid_turn_message_gets_its_own_status_line_and_earlier_lines_stay() {
        use residuum_inference::Role;

        let (interrupt_tx, mut interrupt_rx) = tokio::sync::mpsc::unbounded_channel();
        let provider = CapturingProvider::new(
            vec![
                echo_call(),
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

        let (publisher, ep) = test_bus();
        agent
            .process_message(
                "hello",
                &publisher,
                Some(&ep),
                "",
                Some(&web_origin()),
                &PromptContext::default(),
                &mut interrupt_rx,
                &[],
                None,
                &CancellationToken::new(),
            )
            .await
            .unwrap();

        let calls = captured.lock().await;
        assert_eq!(calls.len(), 2, "provider should be called twice");
        assert_extends(
            &calls[0],
            &calls[1],
            "the injected message is appended; the kickoff's line stays put",
        );
        let injected = calls[1]
            .iter()
            .position(|m| m.content == "actually, do X instead")
            .unwrap();
        let line = &calls[1][injected - 1];
        assert!(is_status_line(line), "a line precedes the injected message");
        assert!(
            line.content.contains("[Last Message:")
                && line.content.ends_with("[Message Source: test]"),
            "its own line, computed at its arrival: {}",
            line.content
        );
        assert_eq!(
            calls[1].iter().filter(|m| is_status_line(m)).count(),
            2,
            "the kickoff and the injected message each carry one"
        );
        assert_eq!(calls[1][injected].role, Role::User);
    }

    #[tokio::test]
    async fn a_rerun_replays_the_status_line_its_message_arrived_with() {
        let (mut agent, requests) = scripted_agent(vec![model_down(), Ok(reply("ok"))]);
        assert!(send(&mut agent, "hello", "t1", &[]).await.is_err());

        retry(&mut agent, "t1").await.unwrap();

        let requests = requests.lock().unwrap().clone();
        assert_eq!(
            request_json(&requests[0].messages),
            request_json(&requests[1].messages),
            "the rerun sends the failed attempt's request unchanged"
        );
    }

    #[tokio::test]
    async fn a_restored_history_replays_its_status_lines_unchanged() {
        use residuum_memory::recent_messages::{append_recent_messages, load_messages_for_agent};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recent_messages.json");
        let (mut before, before_requests) = scripted_agent(vec![Ok(reply("one"))]);
        send(&mut before, "first", "t1", &[]).await.unwrap();
        append_recent_messages(
            &path,
            before.messages_since(0),
            residuum_memory::types::Visibility::User,
            chrono_tz::UTC,
            Some("t1"),
        )
        .await
        .unwrap();

        let restore = load_messages_for_agent(&path).await.unwrap();
        let (mut after, after_requests) = scripted_agent(vec![Ok(reply("two"))]);
        after.restore_messages(restore.messages);
        after.set_last_user_message_at(restore.last_user_message_at);
        send(&mut after, "second", "t2", &[]).await.unwrap();

        let first_request = before_requests.lock().unwrap()[0].messages.clone();
        let restored_request = after_requests.lock().unwrap()[0].messages.clone();
        assert_extends(
            &first_request,
            &restored_request,
            "the restored history replays the first turn's request",
        );
    }

    async fn turn_with_skills(agent: &mut Agent, skills_index: Option<&str>) {
        let (publisher, ep) = test_bus();
        let mut irx = residuum_bus::testing::dead_interrupt_rx();
        let prompt_ctx = PromptContext {
            skills: SkillsContext {
                index: skills_index,
                active_instructions: None,
            },
            ..PromptContext::default()
        };
        agent
            .process_message(
                "hi",
                &publisher,
                Some(&ep),
                "",
                Some(&web_origin()),
                &prompt_ctx,
                &mut irx,
                &[],
                None,
                &CancellationToken::new(),
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_system_prompt_change_between_turns_logs_a_cache_break_once_at_info() {
        let log = residuum_core::testing::log::EventLog::default();
        let _capture = log.capture();
        let (mut agent, _requests) = scripted_agent(vec![
            Ok(reply("one")),
            Ok(reply("two")),
            Ok(reply("three")),
            Ok(reply("four")),
        ]);

        turn_with_skills(&mut agent, None).await;
        turn_with_skills(&mut agent, None).await;
        assert!(
            log.matching("prompt cache prefix changed").is_empty(),
            "a turn whose earlier parts repeat logs no cache break"
        );

        turn_with_skills(&mut agent, Some("<skill name=\"a\"/>")).await;
        turn_with_skills(&mut agent, Some("<skill name=\"b\"/>")).await;

        let breaks = log.matching("prompt cache prefix changed");
        let [first, second] = breaks.as_slice() else {
            panic!("expected one break per skills change, got {breaks:?}");
        };
        assert_eq!(
            first.level,
            tracing::Level::INFO,
            "the first system change between turns logs at info: {first:?}"
        );
        assert!(
            first.text.contains("cause=\"system_changed\"")
                && first.text.contains("part=\"system\""),
            "the break names the system prompt as the changed part: {first:?}"
        );
        assert_eq!(
            second.level,
            tracing::Level::DEBUG,
            "a repeat of the same cause logs at debug: {second:?}"
        );
        assert!(
            second.text.contains("cause=\"system_changed\""),
            "the repeat has the same cause: {second:?}"
        );
    }
}
