//! Session turn execution.

use anyhow::Context as _;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::agent::context::{MemoryContext, PromptContext, SkillsContext};
use crate::agent::interrupt::dead_interrupt_rx;
use crate::agent::recent_messages::RecentMessages;
use crate::agent::turn::{EventContext, TurnResources, execute_turn};
use crate::bus::Publisher;
use crate::inference::{CompletionOptions, InferenceProvider, Message};
use crate::mcp::SharedMcpRegistry;
use crate::memory::merge_writer::MemoryMergeWriter;
use crate::memory::observer::Observer;
use crate::skills::{SharedSkillState, SkillState};
use crate::tools::path_policy::PathPolicy;
use crate::tools::{FileTracker, ToolRegistry};
use crate::workspace::identity::IdentityFiles;

use super::types::{SubAgentBuildConfig, SubAgentConfig};

/// Output from a completed session turn.
pub(crate) struct SubAgentOutput {
    /// The final text response (last assistant message).
    pub summary: String,
    /// Full conversation transcript (all messages exchanged during the turn).
    pub messages: Vec<Message>,
}

/// Everything needed to run a session turn, gathered at fork time.
pub struct SubAgentResources {
    pub(crate) provider: Box<dyn InferenceProvider>,
    pub(crate) tools: ToolRegistry,
    /// Shared MCP registry (ref-counted, not isolated).
    pub(crate) mcp_registry: SharedMcpRegistry,
    /// This session's own isolated skill state.
    pub(crate) skill_state: SharedSkillState,
    pub(crate) identity: IdentityFiles,
    pub(crate) options: CompletionOptions,
    /// Formatted skill index for the system prompt (built at fork time).
    pub(crate) skills_index: Option<String>,
    /// Snapshot of the global observation log, taken at fork time.
    pub(crate) observations: Option<String>,
    /// Snapshot of the recent-context narrative, taken at fork time.
    pub(crate) recent_context: Option<String>,
    /// Workspace layout, for the completion memory pipeline (episode
    /// storage, observer guidance files).
    pub(crate) layout: crate::workspace::layout::WorkspaceLayout,
    /// This session's own observer instance, for per-run threshold checks
    /// and extraction.
    pub(crate) observer: Arc<Observer>,
    /// The single serialized writer for global memory.
    pub(crate) merge_writer: Arc<MemoryMergeWriter>,
    /// Token floor below which a completed run with nothing staged produces
    /// no episode.
    pub(crate) episode_skip_token_floor: usize,
}

/// Build isolated session resources from the main agent's shared state.
///
/// Clones the skill index so the session starts with the same view of
/// available skills, but operates on its own independent copies of
/// `SkillState` and `PathPolicy`. The `McpRegistry` is shared (ref-counted)
/// so servers are not duplicated.
///
/// When `config.skill` is set, that skill is activated on the session's own
/// skill state so its body arrives as the session's role instructions.
///
/// # Errors
/// Returns an error if `config.skill` names a skill that cannot be resolved or
/// read — a session without the instructions that define its job is not worth
/// running, so the spawn fails instead.
#[tracing::instrument(skip_all)]
pub async fn build_subagent_resources(
    provider: Box<dyn InferenceProvider>,
    main_skill_state: &SharedSkillState,
    mcp_registry: SharedMcpRegistry,
    config: SubAgentBuildConfig,
) -> anyhow::Result<SubAgentResources> {
    let SubAgentBuildConfig {
        workspace_layout,
        identity,
        options,
        tz,
        skill,
        observations,
        recent_context,
        session_registry,
        endpoint_registry,
        publisher,
        action_store,
        action_notify,
        hybrid_searcher,
        observer,
        merge_writer,
        episode_skip_token_floor,
    } = config;

    // Clone skill index and dirs for an isolated SkillState (no active skills)
    let (cloned_skill_index, skill_dirs) = {
        let guard = main_skill_state.lock().await;
        (guard.index().clone(), guard.dirs().to_vec())
    };
    let skill_state = SkillState::new_shared(cloned_skill_index, skill_dirs);

    // Activate the requested skill up front so its body renders as this
    // sub-agent's role instructions through the normal active-skill path.
    // A name that doesn't resolve fails the spawn rather than silently
    // running a sub-agent without the instructions that define its job.
    if let Some(name) = &skill {
        let mut guard = skill_state.lock().await;
        guard
            .activate(name)
            .await
            .with_context(|| format!("failed to activate skill '{name}' for sub-agent"))?;
    }

    // Fresh isolated path policy
    let path_policy = PathPolicy::new_shared();

    // Fresh file tracker (tracks reads within this sub-agent turn only)
    let tracker = FileTracker::new_shared();

    // Build the formatted index for the system prompt
    let skills_index = {
        let guard = skill_state.lock().await;
        let idx = guard.format_index_for_prompt();
        if idx.is_empty() { None } else { Some(idx) }
    };

    let tools = ToolRegistry::build_subagent_registry(
        tracker,
        Arc::clone(&path_policy),
        Arc::clone(&skill_state),
        tz,
        hybrid_searcher,
        workspace_layout.episodes_dir(),
        workspace_layout.agent_inbox_dir(),
        workspace_layout.agent_inbox_archive_dir(),
        workspace_layout.user_inbox_dir(),
        workspace_layout.user_inbox_attachments_dir(),
        session_registry,
        endpoint_registry,
        publisher,
        action_store,
        action_notify,
    );

    Ok(SubAgentResources {
        provider,
        tools,
        mcp_registry,
        skill_state,
        identity,
        options,
        skills_index,
        observations,
        recent_context,
        layout: workspace_layout,
        observer,
        merge_writer,
        episode_skip_token_floor,
    })
}

/// Execute one session turn.
///
/// Builds the turn's starting user message from the task prompt and context
/// alone — identity, the observation snapshot, and skills are carried in the
/// system message that `execute_turn` assembles itself, the same way the main
/// agent's turns are, so nothing is injected twice.
///
/// `stop_token` is the session's own token: cancelling it (via `stop_agent`)
/// aborts an in-flight model call or ends the turn at its next checkpoint,
/// leaving `recent_messages` — and therefore the transcript — intact up to
/// that point.
///
/// `transcript_sink`, when given, receives every message as it's produced
/// (the initial user message here, then each model response and tool
/// result inside `execute_turn`) so the run's transcript survives a crash
/// mid-turn in the session store.
///
/// # Errors
/// Returns an error if the model call fails.
#[tracing::instrument(skip_all, fields(run.id = %run_id))]
pub(crate) async fn execute_subagent(
    run_id: &str,
    config: &SubAgentConfig,
    resources: &SubAgentResources,
    stop_token: &CancellationToken,
    transcript_sink: Option<&dyn crate::agent::turn::TranscriptSink>,
) -> Result<SubAgentOutput, anyhow::Error> {
    // Build skills context from this session's isolated skill state
    let active_instructions: Option<String> = {
        let guard = resources.skill_state.lock().await;
        guard.format_active_for_prompt()
    };
    let skills_ctx = SkillsContext {
        index: resources.skills_index.as_deref(),
        active_instructions: active_instructions.as_deref(),
    };

    // Build the user message: source-specific context, then the prompt. No
    // identity/wiki/skills content here — that lives in the system message.
    let mut user_parts = Vec::new();

    if let Some(ctx) = &config.context {
        user_parts.push(ctx.clone());
    }

    user_parts.push(config.prompt.clone());

    let combined_prompt = user_parts.join("\n\n");
    let initial_message = Message::user(combined_prompt);
    let mut recent_messages = RecentMessages::new();
    recent_messages.push(initial_message.clone());
    if let Some(sink) = transcript_sink {
        sink.append(&[initial_message]).await;
    }

    // No broker needed: sessions pass `None` for both endpoints, so
    // streaming events are never published. A noop publisher satisfies
    // the type without spawning a background task.
    let publisher = Publisher::noop();
    let mut interrupt_rx = dead_interrupt_rx();

    let memory_ctx = MemoryContext {
        observations: resources.observations.as_deref(),
        recent_context: resources.recent_context.as_deref(),
    };

    let prompt_ctx = PromptContext { skills: skills_ctx };

    let turn_resources = TurnResources {
        provider: &*resources.provider,
        tools: &resources.tools,
        mcp_registry: &resources.mcp_registry,
        identity: &resources.identity,
        options: &resources.options,
        stop_token,
        transcript_sink,
    };

    let events = EventContext {
        publisher: &publisher,
        output_endpoint: None,
        tool_activity_endpoint: None,
        correlation_id: "",
    };
    // Session turns are not watched by the subconscious (main agent only).
    let mut texts: Vec<String> = execute_turn(
        &turn_resources,
        &memory_ctx,
        &prompt_ctx,
        &mut recent_messages,
        &events,
        None,
        &mut interrupt_rx,
        None,
    )
    .await?;

    if texts.is_empty() {
        tracing::warn!(run_id = %run_id, "session turn produced no text output");
    }
    let summary = texts.pop().unwrap_or_default();
    let messages = recent_messages.messages().to_vec();
    Ok(SubAgentOutput { summary, messages })
}

/// Test-only layout, observer, and merge writer, backed by a leaked temp
/// directory. `Observer::disabled`/a threshold-disabled reflector mean
/// neither ever fires unless a test explicitly configures otherwise. Shared
/// with `runtime`'s tests, which also build `SubAgentResources`.
#[cfg(test)]
pub(crate) fn test_memory_extras() -> (
    crate::workspace::layout::WorkspaceLayout,
    Arc<Observer>,
    Arc<MemoryMergeWriter>,
) {
    let dir = tempfile::tempdir().unwrap().keep();
    let layout = crate::workspace::layout::WorkspaceLayout::new(&dir);
    let search_index = Arc::new(
        crate::memory::search::MemoryIndex::open_or_create(&layout.search_index_dir()).unwrap(),
    );
    let reflector = crate::memory::reflector::Reflector::disabled(chrono_tz::UTC);
    let merge_writer = Arc::new(MemoryMergeWriter::new(
        reflector,
        layout.clone(),
        search_index,
        None,
        None,
    ));
    let observer = Arc::new(Observer::disabled(chrono_tz::UTC));
    (layout, observer, merge_writer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inference::{InferenceError, InferenceResponse, ToolDefinition};
    use crate::mcp::McpRegistry;
    use crate::skills::{SkillIndex, SkillState};
    use async_trait::async_trait;

    struct MockSubAgentProvider {
        response: String,
    }

    #[async_trait]
    impl InferenceProvider for MockSubAgentProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _options: &CompletionOptions,
        ) -> Result<InferenceResponse, InferenceError> {
            Ok(InferenceResponse::new(self.response.clone(), vec![]))
        }

        fn model_name(&self) -> &'static str {
            "mock-subagent"
        }
    }

    fn make_resources(response: &str) -> SubAgentResources {
        let skill_state = SkillState::new_shared(SkillIndex::default(), vec![]);
        let mcp_registry = McpRegistry::new_shared();
        let (layout, observer, merge_writer) = test_memory_extras();
        SubAgentResources {
            provider: Box::new(MockSubAgentProvider {
                response: response.to_string(),
            }),
            tools: ToolRegistry::new(),
            mcp_registry,
            skill_state,
            identity: IdentityFiles::default(),
            options: CompletionOptions::default(),
            skills_index: None,
            observations: None,
            recent_context: None,
            layout,
            observer,
            merge_writer,
            episode_skip_token_floor: 2000,
        }
    }

    #[tokio::test]
    async fn subagent_returns_summary() {
        let resources = make_resources("3 new emails found");

        let config = SubAgentConfig {
            prompt: "check emails".to_string(),
            context: None,
            model_tier: crate::config::BackgroundModelTier::Medium,
        };

        let output = execute_subagent(
            "run-001",
            &config,
            &resources,
            &CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
        assert_eq!(output.summary, "3 new emails found");
    }

    #[tokio::test]
    async fn subagent_captures_full_transcript() {
        let resources = make_resources("done");

        let config = SubAgentConfig {
            prompt: "do work".to_string(),
            context: None,
            model_tier: crate::config::BackgroundModelTier::Small,
        };

        let output = execute_subagent(
            "run-002",
            &config,
            &resources,
            &CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
        assert_eq!(output.summary, "done");
        assert!(
            output.messages.len() >= 2,
            "transcript should contain at least user + assistant messages, got {}",
            output.messages.len()
        );
        let first = output.messages.first().unwrap();
        assert_eq!(first.role, crate::inference::Role::User);
        assert!(
            first.content.contains("do work"),
            "user message should contain the prompt"
        );
        let last = output.messages.last().unwrap();
        assert_eq!(last.role, crate::inference::Role::Assistant);
        assert_eq!(last.content, "done");
    }

    #[tokio::test]
    async fn subagent_user_message_carries_only_context_and_prompt() {
        let resources = make_resources("result");

        let config = SubAgentConfig {
            prompt: "check emails".to_string(),
            context: Some("extra context".to_string()),
            model_tier: crate::config::BackgroundModelTier::Medium,
        };

        let output = execute_subagent(
            "run-ctx",
            &config,
            &resources,
            &CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
        let first = output.messages.first().unwrap();
        assert_eq!(first.role, crate::inference::Role::User);
        assert_eq!(
            first.content, "extra context\n\ncheck emails",
            "the user message must carry only source context and the task prompt — \
             identity, wiki, and skills belong in the system message, assembled once \
             by execute_turn, not duplicated here"
        );
    }

    /// Captures every message list it was called with, so a test can assert on
    /// the assembled system message rather than a canned response.
    struct CapturingProvider {
        response: String,
        seen: Arc<std::sync::Mutex<Vec<Vec<Message>>>>,
    }

    #[async_trait]
    impl InferenceProvider for CapturingProvider {
        async fn complete(
            &self,
            messages: &[Message],
            _tools: &[ToolDefinition],
            _options: &CompletionOptions,
        ) -> Result<InferenceResponse, InferenceError> {
            self.seen.lock().unwrap().push(messages.to_vec());
            Ok(InferenceResponse::new(self.response.clone(), vec![]))
        }

        fn model_name(&self) -> &'static str {
            "capturing"
        }
    }

    #[tokio::test]
    async fn fork_system_message_carries_identity_and_memory_snapshot_exactly_once() {
        let skill_state = SkillState::new_shared(SkillIndex::default(), vec![]);
        let mcp_registry = McpRegistry::new_shared();
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (layout, observer, merge_writer) = test_memory_extras();
        let resources = SubAgentResources {
            provider: Box::new(CapturingProvider {
                response: "done".to_string(),
                seen: Arc::clone(&seen),
            }),
            tools: ToolRegistry::new(),
            mcp_registry,
            skill_state,
            identity: IdentityFiles {
                soul: Some("I am the agent.".to_string()),
                user: Some("User likes Rust.".to_string()),
                wiki_index: Some("wiki catalog".to_string()),
                ..IdentityFiles::default()
            },
            options: CompletionOptions::default(),
            skills_index: None,
            observations: Some("episode ep-1: learned something".to_string()),
            recent_context: Some("we were mid-refactor".to_string()),
            layout,
            observer,
            merge_writer,
            episode_skip_token_floor: 2000,
        };

        let config = SubAgentConfig {
            prompt: "continue the task".to_string(),
            context: None,
            model_tier: crate::config::BackgroundModelTier::Medium,
        };

        execute_subagent(
            "run-fork",
            &config,
            &resources,
            &CancellationToken::new(),
            None,
        )
        .await
        .unwrap();

        let calls = seen.lock().unwrap();
        let call = calls.first().expect("provider should have been called");
        let system = call
            .iter()
            .find(|m| m.role == crate::inference::Role::System)
            .expect("a system message must be present");

        for needle in [
            "I am the agent.",
            "User likes Rust.",
            "wiki catalog",
            "episode ep-1: learned something",
            "we were mid-refactor",
        ] {
            let count = system.content.matches(needle).count();
            assert_eq!(
                count, 1,
                "'{needle}' should appear exactly once, got {count}"
            );
        }

        let user = call
            .iter()
            .find(|m| m.role == crate::inference::Role::User)
            .expect("a user message must be present");
        assert_eq!(user.content, "continue the task");
        assert!(
            !user.content.contains("I am the agent."),
            "identity must not be duplicated into the user message"
        );
    }

    #[tokio::test]
    async fn stopping_a_session_keeps_its_partial_transcript() {
        // A blocking provider paired with a pre-cancelled stop token exercises
        // the same cooperative-cancellation path `stop_agent` uses: the turn
        // ends at its next checkpoint instead of the whole future being
        // dropped, so the user message that was already pushed survives.
        struct BlockingProvider;

        #[async_trait]
        impl InferenceProvider for BlockingProvider {
            async fn complete(
                &self,
                _messages: &[Message],
                _tools: &[ToolDefinition],
                _options: &CompletionOptions,
            ) -> Result<InferenceResponse, InferenceError> {
                std::future::pending().await
            }

            fn model_name(&self) -> &'static str {
                "blocking"
            }
        }

        let skill_state = SkillState::new_shared(SkillIndex::default(), vec![]);
        let mcp_registry = McpRegistry::new_shared();
        let (layout, observer, merge_writer) = test_memory_extras();
        let resources = SubAgentResources {
            provider: Box::new(BlockingProvider),
            tools: ToolRegistry::new(),
            mcp_registry,
            skill_state,
            identity: IdentityFiles::default(),
            options: CompletionOptions::default(),
            skills_index: None,
            observations: None,
            recent_context: None,
            layout,
            observer,
            merge_writer,
            episode_skip_token_floor: 2000,
        };
        let config = SubAgentConfig {
            prompt: "do work".to_string(),
            context: None,
            model_tier: crate::config::BackgroundModelTier::Medium,
        };

        let stop_token = CancellationToken::new();
        stop_token.cancel();

        let output = execute_subagent("run-stop", &config, &resources, &stop_token, None)
            .await
            .unwrap();
        assert!(
            output
                .messages
                .iter()
                .any(|m| m.content.contains("do work")),
            "the pre-turn user message should survive a stop"
        );
    }

    #[tokio::test]
    async fn stopping_a_session_records_the_stop_note_in_the_transcript_sink() {
        // A stop mid-model-call pushes a system "stop note" onto
        // `recent_messages` (see `agent::turn::STOP_NOTE`). It must reach the
        // durable transcript sink the same way every other message in the
        // turn does, not just the in-memory buffer, so a crash right after
        // the stop doesn't lose the note startup recovery relies on.
        struct BlockingProvider;

        #[async_trait]
        impl InferenceProvider for BlockingProvider {
            async fn complete(
                &self,
                _messages: &[Message],
                _tools: &[ToolDefinition],
                _options: &CompletionOptions,
            ) -> Result<InferenceResponse, InferenceError> {
                std::future::pending().await
            }

            fn model_name(&self) -> &'static str {
                "blocking"
            }
        }

        let skill_state = SkillState::new_shared(SkillIndex::default(), vec![]);
        let mcp_registry = McpRegistry::new_shared();
        let (layout, observer, merge_writer) = test_memory_extras();
        let resources = SubAgentResources {
            provider: Box::new(BlockingProvider),
            tools: ToolRegistry::new(),
            mcp_registry,
            skill_state,
            identity: IdentityFiles::default(),
            options: CompletionOptions::default(),
            skills_index: None,
            observations: None,
            recent_context: None,
            layout,
            observer,
            merge_writer,
            episode_skip_token_floor: 2000,
        };
        let config = SubAgentConfig {
            prompt: "do work".to_string(),
            context: None,
            model_tier: crate::config::BackgroundModelTier::Medium,
        };

        let store_dir = tempfile::tempdir().unwrap();
        let store = crate::background::store::SessionStore::new(store_dir.path().to_path_buf());
        let started_at = chrono::Utc::now();
        let sink = crate::background::store::RunTranscriptSink {
            store: &store,
            run_id: "run-stop-note",
            started_at,
        };

        let stop_token = CancellationToken::new();
        stop_token.cancel();

        execute_subagent(
            "run-stop-note",
            &config,
            &resources,
            &stop_token,
            Some(&sink),
        )
        .await
        .unwrap();

        let transcript = store
            .read_incremental_transcript("run-stop-note", started_at)
            .await;
        assert!(
            transcript
                .iter()
                .any(|m| m.content.contains("the user stopped this turn")),
            "the stop note must reach the durable transcript sink, got {transcript:?}"
        );
    }
}
