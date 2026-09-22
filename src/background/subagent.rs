//! Session turn execution.

use anyhow::Context as _;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::agent::context::{MemoryContext, PromptContext, SkillsContext};
use crate::agent::hop::HopCounter;
use crate::agent::interrupt::Interrupt;
use crate::agent::recent_messages::RecentMessages;
use crate::agent::turn::{EventContext, EventTarget, TurnResources, execute_turn};
use crate::bus::{AgentMessageEvent, Publisher, SessionAddress};
use crate::inference::{CompletionOptions, InferenceProvider, Message};
use crate::mcp::SharedMcpRegistry;
use crate::memory::merge_writer::MemoryMergeWriter;
use crate::memory::observer::Observer;
use crate::skills::{SharedSkillState, SkillState};
use crate::tools::path_policy::PathPolicy;
use crate::tools::{FileTracker, ToolRegistry};
use crate::workspace::identity::IdentityFiles;

use super::types::SubAgentBuildConfig;

/// What kicks off one of a session's turns.
///
/// A run's first turn is always [`Self::Initial`] — the source-specific task
/// prompt and context the design calls "fork contents". Every later turn in
/// the same run (the session was idle and an agent message arrived) is
/// [`Self::AgentMessage`] instead: the message becomes the turn's input,
/// formatted with the sender's address and category so the session can
/// reply.
pub(crate) enum TurnKickoff {
    /// The run's first turn: the task prompt, plus any source-specific
    /// context (a pulse/action/webhook payload, or a resume pointer).
    Initial {
        prompt: String,
        context: Option<String>,
        /// Hop count of this run's first turn (see [`super::types::SubAgentConfig::hop_count`]).
        hop_count: u32,
    },
    /// A later turn, started because a message reached this session while it
    /// was idle.
    AgentMessage(AgentMessageEvent),
}

impl TurnKickoff {
    /// This turn's driving hop count, before it's consumed into message text
    /// — the session's hop counter is set to this at the top of the turn.
    fn hop_count(&self) -> u32 {
        match self {
            Self::Initial { hop_count, .. } => *hop_count,
            Self::AgentMessage(msg) => msg.hop_count,
        }
    }

    /// Render this kickoff as the turn's opening user message.
    fn into_message_text(self) -> String {
        match self {
            Self::Initial {
                prompt, context, ..
            } => {
                let mut parts = Vec::new();
                if let Some(ctx) = context {
                    parts.push(ctx);
                }
                parts.push(prompt);
                parts.join("\n\n")
            }
            Self::AgentMessage(msg) => msg.format_for_agent(),
        }
    }
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
    /// This session's current-turn hop counter, shared with its
    /// `message_agent`/`subagent_spawn` tools (see
    /// [`super::types::SubAgentBuildConfig::hop_counter`]).
    pub(crate) hop_counter: HopCounter,
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
        own_address,
        own_depth,
        subagent_depth_cap,
        session_category,
        messenger,
        hop_counter,
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
        workspace_layout.sessions_dir(),
        workspace_layout.agent_inbox_dir(),
        workspace_layout.agent_inbox_archive_dir(),
        workspace_layout.user_inbox_dir(),
        workspace_layout.user_inbox_attachments_dir(),
        session_registry,
        endpoint_registry,
        publisher,
        action_store,
        action_notify,
        own_address,
        own_depth,
        subagent_depth_cap,
        session_category.as_str().to_string(),
        messenger,
        hop_counter.clone(),
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
        hop_counter,
    })
}

/// Which run a session turn belongs to, and the publisher its streaming
/// events (tool activity, intermediate text) go out on, tagged with the
/// session's address and run id.
pub(crate) struct SessionTurnIdentity<'a> {
    pub(crate) publisher: &'a Publisher,
    pub(crate) address: &'a SessionAddress,
    pub(crate) run_id: &'a str,
}

/// Execute one turn of a session's run.
///
/// `recent_messages` is the run's whole history so far — empty on the run's
/// first turn, carrying every prior turn's messages on a later one, so a
/// session that wakes from idle to handle an agent message still has its
/// earlier context. `kickoff` becomes this turn's opening user message:
/// identity, the observation snapshot, and skills are carried in the system
/// message that `execute_turn` assembles itself, the same way the main
/// agent's turns are, so nothing is injected twice.
///
/// `stop_token` is the session's own token: cancelling it (via `stop_agent`)
/// aborts an in-flight model call or ends the turn at its next checkpoint,
/// leaving `recent_messages` — and therefore the transcript — intact up to
/// that point.
///
/// `transcript_sink`, when given, receives every message as it's produced
/// (the kickoff message here, then each model response and tool result
/// inside `execute_turn`) so the run's transcript survives a crash mid-turn
/// in the session store.
///
/// `interrupt_rx` is the run's own long-lived interrupt channel: draining it
/// here is what delivers an agent message to a *running* turn at its next
/// tool-call boundary. Between turns, the caller drains the same channel
/// itself to decide whether to wake for another turn (see
/// `crate::background::runtime`).
///
/// Returns this turn's final text response. The full transcript is left in
/// `recent_messages` for the caller.
///
/// # Errors
/// Returns an error if the model call fails.
#[tracing::instrument(skip_all, fields(run.id = %identity.run_id))]
pub(crate) async fn execute_subagent(
    identity: &SessionTurnIdentity<'_>,
    kickoff: TurnKickoff,
    recent_messages: &mut RecentMessages,
    resources: &SubAgentResources,
    stop_token: &CancellationToken,
    transcript_sink: Option<&dyn crate::agent::turn::TranscriptSink>,
    interrupt_rx: &mut mpsc::Receiver<Interrupt>,
) -> Result<String, anyhow::Error> {
    // Build skills context from this session's isolated skill state
    let active_instructions: Option<String> = {
        let guard = resources.skill_state.lock().await;
        guard.format_active_for_prompt()
    };
    let skills_ctx = SkillsContext {
        index: resources.skills_index.as_deref(),
        active_instructions: active_instructions.as_deref(),
    };

    // Reset the run's hop counter to this turn's driving input before it's
    // consumed into message text — later `Interrupt::AgentMessage`s drained
    // during the turn raise it further (see `agent::turn::drain_interrupts`).
    resources.hop_counter.set(kickoff.hop_count());

    // No identity/wiki/skills content here — that lives in the system message.
    let kickoff_message = Message::user(kickoff.into_message_text());
    recent_messages.push(kickoff_message.clone());
    if let Some(sink) = transcript_sink {
        sink.append(&[kickoff_message]).await;
    }

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
        hop_counter: &resources.hop_counter,
    };

    let events = EventContext {
        publisher: identity.publisher,
        target: EventTarget::Session {
            address: identity.address,
            run_id: identity.run_id,
        },
    };
    // Session turns are not watched by the subconscious (main agent only).
    let mut texts: Vec<String> = execute_turn(
        &turn_resources,
        &memory_ctx,
        &prompt_ctx,
        recent_messages,
        &events,
        None,
        interrupt_rx,
        None,
    )
    .await?;

    if texts.is_empty() {
        tracing::warn!(run_id = %identity.run_id, "session turn produced no text output");
    }
    Ok(texts.pop().unwrap_or_default())
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
    use crate::agent::interrupt::dead_interrupt_rx;
    use crate::inference::{InferenceError, InferenceResponse, ToolDefinition};
    use crate::mcp::McpRegistry;
    use crate::skills::{SkillIndex, SkillState};
    use async_trait::async_trait;

    /// A turn identity with no broker behind it: these tests exercise the
    /// turn itself, not the events it publishes (see the runtime's tests).
    fn test_identity(run_id: &'static str) -> SessionTurnIdentity<'static> {
        SessionTurnIdentity {
            publisher: Box::leak(Box::new(Publisher::noop())),
            address: Box::leak(Box::new(SessionAddress::from("spawned-test-0001"))),
            run_id,
        }
    }

    fn initial(prompt: &str, context: Option<&str>) -> TurnKickoff {
        TurnKickoff::Initial {
            prompt: prompt.to_string(),
            context: context.map(str::to_string),
            hop_count: 0,
        }
    }

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
            hop_counter: crate::agent::HopCounter::new(0),
        }
    }

    #[tokio::test]
    async fn subagent_returns_summary() {
        let resources = make_resources("3 new emails found");
        let mut recent_messages = RecentMessages::new();
        let mut interrupt_rx = dead_interrupt_rx();

        let summary = execute_subagent(
            &test_identity("run-001"),
            initial("check emails", None),
            &mut recent_messages,
            &resources,
            &CancellationToken::new(),
            None,
            &mut interrupt_rx,
        )
        .await
        .unwrap();
        assert_eq!(summary, "3 new emails found");
    }

    #[tokio::test]
    async fn subagent_captures_full_transcript() {
        let resources = make_resources("done");
        let mut recent_messages = RecentMessages::new();
        let mut interrupt_rx = dead_interrupt_rx();

        let summary = execute_subagent(
            &test_identity("run-002"),
            initial("do work", None),
            &mut recent_messages,
            &resources,
            &CancellationToken::new(),
            None,
            &mut interrupt_rx,
        )
        .await
        .unwrap();
        assert_eq!(summary, "done");
        let messages = recent_messages.messages();
        assert!(
            messages.len() >= 2,
            "transcript should contain at least user + assistant messages, got {}",
            messages.len()
        );
        let first = messages.first().unwrap();
        assert_eq!(first.role, crate::inference::Role::User);
        assert!(
            first.content.contains("do work"),
            "user message should contain the prompt"
        );
        let last = messages.last().unwrap();
        assert_eq!(last.role, crate::inference::Role::Assistant);
        assert_eq!(last.content, "done");
    }

    #[tokio::test]
    async fn subagent_user_message_carries_only_context_and_prompt() {
        let resources = make_resources("result");
        let mut recent_messages = RecentMessages::new();
        let mut interrupt_rx = dead_interrupt_rx();

        execute_subagent(
            &test_identity("run-ctx"),
            initial("check emails", Some("extra context")),
            &mut recent_messages,
            &resources,
            &CancellationToken::new(),
            None,
            &mut interrupt_rx,
        )
        .await
        .unwrap();
        let first = recent_messages.messages().first().unwrap();
        assert_eq!(first.role, crate::inference::Role::User);
        assert_eq!(
            first.content, "extra context\n\ncheck emails",
            "the user message must carry only source context and the task prompt — \
             identity, wiki, and skills belong in the system message, assembled once \
             by execute_turn, not duplicated here"
        );
    }

    #[tokio::test]
    async fn subagent_agent_message_kickoff_names_sender_and_category() {
        let resources = make_resources("ack");
        let mut recent_messages = RecentMessages::new();
        let mut interrupt_rx = dead_interrupt_rx();

        execute_subagent(
            &test_identity("run-msg"),
            TurnKickoff::AgentMessage(AgentMessageEvent {
                from: crate::bus::SessionAddress::from("main"),
                from_category: "main".to_string(),
                content: "how's it going?".to_string(),
                hop_count: 0,
            }),
            &mut recent_messages,
            &resources,
            &CancellationToken::new(),
            None,
            &mut interrupt_rx,
        )
        .await
        .unwrap();

        let first = recent_messages.messages().first().unwrap();
        assert_eq!(
            first.content,
            "[Agent Message from main (main)]\nhow's it going?"
        );
    }

    #[tokio::test]
    async fn queued_agent_message_is_drained_within_the_same_running_turn() {
        // A message already sitting in the interrupt channel before the
        // turn's tool loop starts is drained at the loop's first checkpoint
        // — the same "next tool-call boundary" delivery a message arriving
        // mid-turn gets. This is what distinguishes delivery to a *running*
        // session from delivery to an *idle* one: no second
        // `execute_subagent` call, no second kickoff — it lands inside the
        // run's existing turn.
        let resources = make_resources("wrapping up");

        let (tx, mut rx) = mpsc::channel(4);
        tx.try_send(Interrupt::AgentMessage(AgentMessageEvent {
            from: crate::bus::SessionAddress::from("main"),
            from_category: "main".to_string(),
            content: "any updates?".to_string(),
            hop_count: 0,
        }))
        .unwrap();

        let mut recent_messages = RecentMessages::new();
        let summary = execute_subagent(
            &test_identity("run-interrupt"),
            initial("keep working", None),
            &mut recent_messages,
            &resources,
            &CancellationToken::new(),
            None,
            &mut rx,
        )
        .await
        .unwrap();

        assert_eq!(summary, "wrapping up");
        let messages = recent_messages.messages();
        assert!(
            messages.iter().any(|m| m.content.contains("any updates?")),
            "the queued agent message should be injected into the running turn, got {messages:?}"
        );
        assert_eq!(
            messages.first().unwrap().content,
            "keep working",
            "the original kickoff should still be the turn's first message"
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
            hop_counter: crate::agent::HopCounter::new(0),
        };

        let mut recent_messages = RecentMessages::new();
        let mut interrupt_rx = dead_interrupt_rx();
        execute_subagent(
            &test_identity("run-fork"),
            initial("continue the task", None),
            &mut recent_messages,
            &resources,
            &CancellationToken::new(),
            None,
            &mut interrupt_rx,
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
            hop_counter: crate::agent::HopCounter::new(0),
        };
        let stop_token = CancellationToken::new();
        stop_token.cancel();

        let mut recent_messages = RecentMessages::new();
        let mut interrupt_rx = dead_interrupt_rx();
        execute_subagent(
            &test_identity("run-stop"),
            initial("do work", None),
            &mut recent_messages,
            &resources,
            &stop_token,
            None,
            &mut interrupt_rx,
        )
        .await
        .unwrap();
        assert!(
            recent_messages
                .messages()
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
            hop_counter: crate::agent::HopCounter::new(0),
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

        let mut recent_messages = RecentMessages::new();
        let mut interrupt_rx = dead_interrupt_rx();
        execute_subagent(
            &test_identity("run-stop-note"),
            initial("do work", None),
            &mut recent_messages,
            &resources,
            &stop_token,
            Some(&sink),
            &mut interrupt_rx,
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
