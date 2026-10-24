//! Session turn execution.

use anyhow::Context as _;
use std::sync::Arc;
#[cfg(test)]
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use residuum_agent::ToolRegistry;
use residuum_agent::context::{MemoryContext, PromptContext, SkillsContext};
use residuum_agent::hop::HopCounter;
use residuum_agent::recent_messages::RecentMessages;
use residuum_agent::turn::{
    EventContext, EventTarget, SessionConversationTarget, TurnResources, execute_turn,
};
use residuum_bus::MessageEvent;
#[cfg(test)]
use residuum_bus::interrupt::Interrupt;
use residuum_bus::{AgentMessageEvent, Publisher, SessionAddress};
use residuum_inference::{CompletionOptions, ImageData, InferenceProvider, Message, MessageSender};
use residuum_mcp::SharedMcpRegistry;
use residuum_memory::merge_writer::MemoryMergeWriter;
use residuum_memory::observer::Observer;
use residuum_skills::{SharedSkillState, SkillState};
use residuum_workspace::identity::IdentityFiles;

use super::fork_failure::ForkFailure;
use super::session_tools::SessionToolScope;
use super::types::SubAgentBuildConfig;

/// What kicks off one of a session's turns.
///
/// A run's first turn is always [`Self::Initial`], unless the session was
/// started by a conversation message, in which case it's [`Self::External`]
/// from the start too. Every later turn in the same run started because a
/// message reached the session while it was idle is either
/// [`Self::AgentMessage`] (another agent addressed it) or [`Self::External`]
/// (a new message arrived in its conversation).
pub(crate) enum TurnKickoff {
    /// The run's first turn: the task prompt, plus any source-specific
    /// context (a pulse/action/webhook payload, or a resume pointer).
    Initial {
        prompt: String,
        context: Option<String>,
        /// Hop count of this run's first turn (see [`super::types::SubAgentConfig::hop_count`]).
        hop_count: u32,
        /// Who sent this kickoff, for a conversation-triggered session —
        /// attributes the opening message the same way main attributes an
        /// inbound chat message. `None` for every other trigger, which keeps
        /// the original single-message rendering (no attribution line).
        sender: Option<MessageSender>,
        /// Images attached to the triggering message, for a
        /// conversation-triggered spawn or resume. Empty for every other
        /// trigger.
        images: Vec<ImageData>,
        /// The agent messages a resume delivers: the turn opens with each
        /// as its own entry, after `context` and ahead of `prompt` (which
        /// then holds only the resume's chat messages, combined, or is
        /// empty), and reports each as read. Empty for every other trigger.
        delivered: Vec<AgentMessageEvent>,
        /// Whether this is a resume whose `prompt` is the chat messages it
        /// delivers, combined (see
        /// [`super::types::SubAgentConfig::delivered_chat_messages`]). Such a
        /// resume opens with `context` as its own entry, as one with
        /// `delivered` messages does, even when it delivers no agent
        /// messages. `false` for every other trigger.
        resumed_by_chat: bool,
    },
    /// A later turn, started because another agent's message reached this
    /// session while it was idle.
    AgentMessage(AgentMessageEvent),
    /// A turn — the run's first, or a later one reached while idle — kicked
    /// off by an inbound conversation message: sender attribution and any
    /// buffered context arrive with it exactly as they do for the main
    /// agent (see [`MessageEvent::into_history_messages`]). Always hop
    /// count 0: inbound conversation messages are external input.
    External(MessageEvent),
}

impl TurnKickoff {
    /// This turn's driving hop count, before it's consumed into message text
    /// — the session's hop counter is set to this at the top of the turn.
    fn hop_count(&self) -> u32 {
        match self {
            Self::Initial { hop_count, .. } => *hop_count,
            Self::AgentMessage(msg) => msg.hop_count,
            Self::External(_) => 0,
        }
    }

    /// The messages from other agents, the owner or an artifact this
    /// kickoff hands the turn, which the turn reports as read once it has
    /// started.
    pub(crate) fn delivered_messages(&self) -> &[AgentMessageEvent] {
        match self {
            Self::Initial { delivered, .. } => delivered,
            Self::AgentMessage(msg) => std::slice::from_ref(msg),
            Self::External(_) => &[],
        }
    }

    /// Render this kickoff as the turn's opening message(s).
    fn into_messages(self) -> Vec<Message> {
        match self {
            Self::Initial {
                prompt,
                context,
                sender,
                images,
                delivered,
                resumed_by_chat,
                ..
            } if !delivered.is_empty() || resumed_by_chat => {
                // A resume by messages: the pointer to the previous run, then
                // each agent message as its own entry with its own id and
                // sender, then the chat messages that came with them.
                let mut msgs: Vec<Message> = context
                    .into_iter()
                    .map(|ctx| Message::user(ctx).as_context())
                    .collect();
                msgs.extend(delivered.iter().map(AgentMessageEvent::to_history_message));
                if !prompt.is_empty() || !images.is_empty() {
                    let user = if images.is_empty() {
                        Message::user(prompt)
                    } else {
                        Message::user_with_images(prompt, images)
                    };
                    msgs.push(user.with_sender(sender));
                }
                msgs
            }
            Self::Initial {
                prompt,
                context,
                sender: None,
                images,
                ..
            } => {
                // No sender: preserve the original single-message rendering
                // exactly, for every trigger that isn't a conversation
                // (pulses, actions, webhooks, agent spawns).
                let mut parts = Vec::new();
                if let Some(ctx) = context {
                    parts.push(ctx);
                }
                parts.push(prompt);
                let text = parts.join("\n\n");
                let user = if images.is_empty() {
                    Message::user(text)
                } else {
                    Message::user_with_images(text, images)
                };
                vec![user]
            }
            Self::Initial {
                prompt,
                context,
                sender: Some(sender),
                images,
                ..
            } => {
                let mut msgs = Vec::new();
                if let Some(ctx) = context {
                    msgs.push(Message::system(ctx).as_context());
                }
                let user = if images.is_empty() {
                    Message::user(prompt)
                } else {
                    Message::user_with_images(prompt, images)
                };
                msgs.push(user.with_sender(Some(sender)));
                msgs
            }
            Self::AgentMessage(msg) => vec![msg.to_history_message()],
            Self::External(inbound) => inbound.into_history_messages(),
        }
    }
}

/// Everything needed to run a session turn, gathered at fork time.
pub struct SubAgentResources {
    pub provider: Box<dyn InferenceProvider>,
    pub tools: ToolRegistry,
    /// Shared MCP registry (ref-counted, not isolated).
    pub mcp_registry: SharedMcpRegistry,
    /// This session's own isolated skill state.
    pub skill_state: SharedSkillState,
    pub identity: IdentityFiles,
    pub options: CompletionOptions,
    /// Maximum tool-call iterations for this session's turns before the turn
    /// stops itself gracefully. `None` means unlimited — see
    /// [`residuum_config::AgentAbilitiesConfig::max_tool_iterations`].
    pub max_tool_iterations: Option<usize>,
    /// Guards against a model repeating the exact same tool call — see
    /// [`residuum_config::AgentAbilitiesConfig::repeat_call_guard`].
    pub repeat_call_guard: residuum_config::RepeatCallGuardConfig,
    /// Formatted skill index for the system prompt (built at fork time).
    pub skills_index: Option<String>,
    /// Snapshot of the global observation log, taken at fork time.
    pub observations: Option<String>,
    /// Snapshot of the recent-context narrative, taken at fork time.
    pub recent_context: Option<String>,
    /// Workspace layout, for the completion memory pipeline (episode
    /// storage, observer guidance files).
    pub layout: residuum_config::WorkspaceLayout,
    /// This session's own observer instance, for per-run threshold checks
    /// and extraction.
    pub observer: Arc<Observer>,
    /// The single serialized writer for global memory.
    pub merge_writer: Arc<MemoryMergeWriter>,
    /// Token floor below which a completed run with nothing staged produces
    /// no episode.
    pub episode_skip_token_floor: usize,
    /// This session's current-turn hop counter, shared with its
    /// `message_agent`/`subagent_spawn` tools (see
    /// [`super::types::SubAgentBuildConfig::hop_counter`]).
    pub hop_counter: HopCounter,
}

/// Build an isolated `SkillState` for a new sub-agent, cloned from
/// `main_skill_state`'s index and dirs, with `skill` (if any) activated up
/// front so its body renders as the sub-agent's role instructions through
/// the normal active-skill path. Also returns the formatted skill index for
/// the system prompt. Split out of [`build_subagent_resources`] to keep that
/// function's line count down.
///
/// # Errors
/// Returns an error if `skill` names a skill that cannot be resolved or
/// read — a session without the instructions that define its job is not
/// worth running, so the spawn fails instead.
async fn prepare_session_skill_state(
    main_skill_state: &SharedSkillState,
    skill: Option<&str>,
) -> anyhow::Result<(SharedSkillState, Option<String>)> {
    let (cloned_skill_index, skill_dirs) = {
        let guard = main_skill_state.lock().await;
        (guard.index().clone(), guard.dirs().to_vec())
    };
    let skill_state = SkillState::new_shared(cloned_skill_index, skill_dirs);

    if let Some(name) = skill {
        let mut guard = skill_state.lock().await;
        guard
            .activate(name)
            .await
            .with_context(|| ForkFailure::Skill {
                name: name.to_string(),
            })?;
    }

    let skills_index = {
        let guard = skill_state.lock().await;
        let idx = guard.format_index_for_prompt();
        if idx.is_empty() { None } else { Some(idx) }
    };

    Ok((skill_state, skills_index))
}

/// Build isolated session resources from the main agent's shared state.
///
/// Clones the skill index so the session starts with the same view of
/// available skills, but operates on its own independent copy of
/// `SkillState`. The `McpRegistry` is shared (ref-counted) with the main
/// agent, so servers are not duplicated. The session's tools come from
/// `config.session_tools`, scoped to this session and its own skill state.
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
        max_tool_iterations,
        repeat_call_guard,
        skill,
        observations,
        recent_context,
        observer,
        merge_writer,
        episode_skip_token_floor,
        session_tools,
        own_address,
        own_depth,
        session_category,
        trigger,
        conversation_target,
        hop_counter,
    } = config;

    let (skill_state, skills_index) =
        prepare_session_skill_state(main_skill_state, skill.as_deref()).await?;

    let tools = session_tools.build(SessionToolScope {
        own_address,
        own_depth,
        category: session_category,
        trigger,
        conversation_target,
        hop_counter: hop_counter.clone(),
        skill_state: Arc::clone(&skill_state),
    });

    Ok(SubAgentResources {
        max_tool_iterations,
        repeat_call_guard,
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
    /// Identifies this turn within the run (the id its `TurnStarted` carries).
    pub(crate) turn_id: &'a str,
}

/// Where a conversation session additionally delivers its intermediate turn
/// text: the interface endpoint its conversation lives on, and the
/// conversation id itself. Reuses `identity`'s address and publisher rather
/// than carrying its own — every session already has both for the
/// unconditional session-stream event, so a conversation session's extra
/// chat delivery only needs to name the conversation.
pub(crate) struct ConversationOutput<'a> {
    pub(crate) endpoint: &'a str,
    pub(crate) conversation_id: &'a str,
}

/// A turn's control-flow handles — cancellation, transcript persistence, and
/// the run's own interrupt channel — grouped into one argument so
/// `execute_subagent`'s parameter count stays within clippy's limit.
pub(crate) struct TurnExecution<'a> {
    /// The session's own token: cancelling it (via `stop_agent`) aborts an
    /// in-flight model call or ends the turn at its next checkpoint, leaving
    /// `recent_messages` — and therefore the transcript — intact up to that
    /// point.
    pub(crate) stop_token: &'a CancellationToken,
    /// When given, receives every message as it's produced (the kickoff
    /// message, then each model response and tool result inside
    /// `execute_turn`) so the run's transcript survives a crash mid-turn in
    /// the session store.
    pub(crate) transcript_sink: Option<&'a dyn residuum_agent::turn::TranscriptSink>,
    /// Where this turn's model-call usage accumulates, for the session panel's
    /// spent tokens. `None` for a turn that doesn't track session-level totals.
    pub(crate) usage_sink: Option<&'a dyn residuum_agent::usage::UsageSink>,
    /// The run's own long-lived interrupt channel: draining it is what
    /// delivers an agent message to a *running* turn at its next tool-call
    /// boundary. Between turns, the caller drains the same channel itself to
    /// decide whether to wake for another turn (see
    /// `crate::runtime`).
    pub(crate) interrupt_rx: &'a mut dyn residuum_bus::interrupt::InterruptSource,
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
/// `turn` groups this turn's control-flow handles — see
/// [`TurnExecution`]'s field docs for what each one does.
///
/// `conversation_output`, when given, is where this turn's intermediate
/// (pre-tool-call) text is additionally delivered — a conversation
/// session's own conversation, alongside the session-stream event every
/// session's turn publishes via `identity`. `None` for every other session
/// category: they have no conversation of their own to send it to.
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
    turn: TurnExecution<'_>,
    conversation_output: Option<ConversationOutput<'_>>,
) -> Result<String, anyhow::Error> {
    let TurnExecution {
        stop_token,
        transcript_sink,
        usage_sink,
        interrupt_rx,
    } = turn;
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

    let delivered = kickoff.delivered_messages().to_vec();
    // No identity/wiki/skills content here — that lives in the system message.
    let kickoff_messages = kickoff.into_messages();
    recent_messages.extend(kickoff_messages.clone());
    if let Some(sink) = transcript_sink {
        sink.append(&kickoff_messages).await;
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
        max_tool_iterations: resources.max_tool_iterations,
        repeat_call_guard: resources.repeat_call_guard,
        stop_token,
        transcript_sink,
        usage_sink,
        hop_counter: &resources.hop_counter,
    };

    let events = EventContext {
        publisher: identity.publisher,
        reply_audience: None,
        target: EventTarget::Session {
            address: identity.address,
            run_id: identity.run_id,
            turn_id: identity.turn_id,
        },
        session_conversation: conversation_output.map(|out| SessionConversationTarget {
            session_address: identity.address,
            endpoint: out.endpoint,
            conversation_id: out.conversation_id,
        }),
    };
    // The messages that started this turn are read now: they are in the
    // transcript, and nothing else has been published since `TurnStarted`.
    for msg in &delivered {
        events.publish_message_read(msg).await;
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::test_memory_extras;
    use async_trait::async_trait;
    use residuum_bus::testing::dead_interrupt_rx;
    use residuum_bus::testing::next_event;
    use residuum_config::SkillDir;
    use residuum_inference::{InferenceError, InferenceResponse, ToolDefinition};
    use residuum_mcp::McpRegistry;
    use residuum_skills::{SkillIndex, SkillState};

    /// A turn identity with no broker behind it: these tests exercise the
    /// turn itself, not the events it publishes (see the runtime's tests).
    fn test_identity(run_id: &'static str) -> SessionTurnIdentity<'static> {
        SessionTurnIdentity {
            publisher: Box::leak(Box::new(Publisher::noop())),
            address: Box::leak(Box::new(SessionAddress::from("spawned-test-0001"))),
            run_id,
            turn_id: "run-t1",
        }
    }

    fn initial(prompt: &str, context: Option<&str>) -> TurnKickoff {
        TurnKickoff::Initial {
            prompt: prompt.to_string(),
            context: context.map(str::to_string),
            hop_count: 0,
            sender: None,
            images: Vec::new(),
            delivered: Vec::new(),
            resumed_by_chat: false,
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
            max_tool_iterations: None,
            repeat_call_guard: residuum_config::RepeatCallGuardConfig::default(),
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
            hop_counter: residuum_agent::HopCounter::new(0),
        }
    }

    #[tokio::test]
    async fn a_skill_that_does_not_resolve_fails_the_fork_as_a_skill_failure() {
        let main_state = SkillState::new_shared(SkillIndex::default(), vec![]);

        let Err(err) = prepare_session_skill_state(&main_state, Some("ghost")).await else {
            panic!("a skill that isn't installed must not activate");
        };
        assert_eq!(
            err.downcast_ref::<ForkFailure>(),
            Some(&ForkFailure::Skill {
                name: "ghost".to_string()
            }),
            "the owner is told which skill couldn't load"
        );
    }

    /// A skill that lives only in `team/skills/` resolves as a session role:
    /// sub-agent spawns, pulses (`agent: <skill>`), and artifact sessions all
    /// build their skill state here.
    #[tokio::test]
    async fn team_skill_activates_as_session_role() {
        let agent_skills = tempfile::tempdir().unwrap();
        let team_skills = tempfile::tempdir().unwrap();
        let role_dir = team_skills.path().join("team-role");
        tokio::fs::create_dir_all(&role_dir).await.unwrap();
        tokio::fs::write(
            role_dir.join("SKILL.md"),
            "---\nname: team-role\ndescription: shared role\n---\n\nTeam role instructions.\n",
        )
        .await
        .unwrap();

        let dirs = vec![
            SkillDir::agent(agent_skills.path()),
            SkillDir::team(team_skills.path()),
        ];
        let index = SkillIndex::scan(&dirs).await.unwrap();
        let main_state = SkillState::new_shared(index, dirs);

        let (session_state, skills_index) =
            prepare_session_skill_state(&main_state, Some("team-role"))
                .await
                .unwrap();

        let active = session_state
            .lock()
            .await
            .format_active_for_prompt()
            .unwrap();
        assert!(
            active.contains("Team role instructions."),
            "team skill body should be the session's role: {active}"
        );
        let skills_index = skills_index.unwrap();
        assert!(
            skills_index.contains("<layer>team</layer>"),
            "index should label the team skill: {skills_index}"
        );
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
            TurnExecution {
                stop_token: &CancellationToken::new(),
                transcript_sink: None,
                usage_sink: None,
                interrupt_rx: &mut interrupt_rx,
            },
            None,
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
            TurnExecution {
                stop_token: &CancellationToken::new(),
                transcript_sink: None,
                usage_sink: None,
                interrupt_rx: &mut interrupt_rx,
            },
            None,
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
        assert_eq!(first.role, residuum_inference::Role::User);
        assert!(
            first.content.contains("do work"),
            "user message should contain the prompt"
        );
        let last = messages.last().unwrap();
        assert_eq!(last.role, residuum_inference::Role::Assistant);
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
            TurnExecution {
                stop_token: &CancellationToken::new(),
                transcript_sink: None,
                usage_sink: None,
                interrupt_rx: &mut interrupt_rx,
            },
            None,
        )
        .await
        .unwrap();
        let first = recent_messages.messages().first().unwrap();
        assert_eq!(first.role, residuum_inference::Role::User);
        assert_eq!(
            first.content, "extra context\n\ncheck emails",
            "the user message must carry only source context and the task prompt — \
             identity, wiki, and skills belong in the system message, assembled once \
             by execute_turn, not duplicated here"
        );
        assert!(!first.context, "a task's own framing is not context");
    }

    #[tokio::test]
    async fn subagent_agent_message_kickoff_names_sender_and_category() {
        let resources = make_resources("ack");
        let mut recent_messages = RecentMessages::new();
        let mut interrupt_rx = dead_interrupt_rx();

        execute_subagent(
            &test_identity("run-msg"),
            TurnKickoff::AgentMessage(AgentMessageEvent::new(
                residuum_bus::SessionAddress::from("main"),
                "main",
                "how's it going?",
                0,
            )),
            &mut recent_messages,
            &resources,
            TurnExecution {
                stop_token: &CancellationToken::new(),
                transcript_sink: None,
                usage_sink: None,
                interrupt_rx: &mut interrupt_rx,
            },
            None,
        )
        .await
        .unwrap();

        let first = recent_messages.messages().first().unwrap();
        assert_eq!(
            first.content,
            "[Agent Message from main (main)]\nhow's it going?"
        );
    }

    fn sample_sender() -> MessageSender {
        MessageSender {
            name: "Jane".to_string(),
            id: "discord-jane".to_string(),
            interface: "discord".to_string(),
            location: Some("#builds".to_string()),
        }
    }

    #[tokio::test]
    async fn initial_kickoff_with_a_sender_carries_attribution_not_joined_text() {
        let resources = make_resources("ack");
        let mut recent_messages = RecentMessages::new();
        let mut interrupt_rx = dead_interrupt_rx();

        execute_subagent(
            &test_identity("run-conv-initial"),
            TurnKickoff::Initial {
                prompt: "can you look at this?".to_string(),
                context: None,
                hop_count: 0,
                sender: Some(sample_sender()),
                images: Vec::new(),
                delivered: Vec::new(),
                resumed_by_chat: false,
            },
            &mut recent_messages,
            &resources,
            TurnExecution {
                stop_token: &CancellationToken::new(),
                transcript_sink: None,
                usage_sink: None,
                interrupt_rx: &mut interrupt_rx,
            },
            None,
        )
        .await
        .unwrap();

        let first = recent_messages.messages().first().unwrap();
        // Attribution is structured metadata (`sender`), not baked into
        // `content` — the raw prompt stays clean, matching how the main
        // agent stores its own attributed messages.
        assert_eq!(first.content, "can you look at this?");
        assert_eq!(first.sender.as_ref().map(|s| s.name.as_str()), Some("Jane"));
    }

    #[tokio::test]
    async fn initial_kickoff_with_a_sender_and_context_splits_into_two_messages() {
        let resources = make_resources("ack");
        let mut recent_messages = RecentMessages::new();
        let mut interrupt_rx = dead_interrupt_rx();

        execute_subagent(
            &test_identity("run-conv-initial-ctx"),
            TurnKickoff::Initial {
                prompt: "thoughts?".to_string(),
                context: Some("[14:00] Sam: build is red".to_string()),
                hop_count: 0,
                sender: Some(sample_sender()),
                images: Vec::new(),
                delivered: Vec::new(),
                resumed_by_chat: false,
            },
            &mut recent_messages,
            &resources,
            TurnExecution {
                stop_token: &CancellationToken::new(),
                transcript_sink: None,
                usage_sink: None,
                interrupt_rx: &mut interrupt_rx,
            },
            None,
        )
        .await
        .unwrap();

        let messages = recent_messages.messages();
        let [context, user, ..] = messages else {
            panic!("expected context then user message, got {messages:?}");
        };
        assert_eq!(context.role, residuum_inference::Role::System);
        assert_eq!(context.content, "[14:00] Sam: build is red");
        assert!(context.context, "the context entry is marked as context");
        assert_eq!(user.role, residuum_inference::Role::User);
        assert_eq!(user.content, "thoughts?");
        assert!(!user.context, "the message itself is not context");
    }

    #[tokio::test]
    async fn initial_kickoff_with_images_attaches_them_to_the_user_message() {
        // Regression test: a conversation spawn's or resume's first turn
        // must not silently drop images attached to the triggering message.
        let resources = make_resources("ack");
        let mut recent_messages = RecentMessages::new();
        let mut interrupt_rx = dead_interrupt_rx();
        let image = ImageData {
            media_type: "image/png".to_string(),
            data: "base64-data".to_string(),
        };

        execute_subagent(
            &test_identity("run-conv-initial-images"),
            TurnKickoff::Initial {
                prompt: "what's in this screenshot?".to_string(),
                context: None,
                hop_count: 0,
                sender: None,
                images: vec![image.clone()],
                delivered: Vec::new(),
                resumed_by_chat: false,
            },
            &mut recent_messages,
            &resources,
            TurnExecution {
                stop_token: &CancellationToken::new(),
                transcript_sink: None,
                usage_sink: None,
                interrupt_rx: &mut interrupt_rx,
            },
            None,
        )
        .await
        .unwrap();

        let first = recent_messages.messages().first().unwrap();
        assert_eq!(first.content, "what's in this screenshot?");
        assert_eq!(first.images.len(), 1);
        assert_eq!(first.images.first().unwrap().data, image.data);
    }

    #[tokio::test]
    async fn a_resume_by_several_messages_opens_with_an_entry_for_each() {
        let resources = make_resources("ack");
        let mut recent_messages = RecentMessages::new();
        let mut interrupt_rx = dead_interrupt_rx();
        let owner = AgentMessageEvent::new(
            SessionAddress::from(residuum_bus::session::OWNER_ADDRESS),
            "owner",
            "are you there?",
            0,
        )
        .with_id("m-owner");
        let teammate = AgentMessageEvent::new(
            SessionAddress::from("agent:ivan"),
            residuum_bus::session::TEAMMATE_SENDER_CATEGORY,
            "here's the file",
            1,
        )
        .with_id("m-teammate");

        execute_subagent(
            &test_identity("run-resume-several"),
            TurnKickoff::Initial {
                prompt: "[From: Jane via discord (#builds)] any news?".to_string(),
                context: Some("[Resumed session] previous run".to_string()),
                hop_count: 1,
                sender: None,
                images: Vec::new(),
                delivered: vec![owner.clone(), teammate.clone()],
                resumed_by_chat: true,
            },
            &mut recent_messages,
            &resources,
            TurnExecution {
                stop_token: &CancellationToken::new(),
                transcript_sink: None,
                usage_sink: None,
                interrupt_rx: &mut interrupt_rx,
            },
            None,
        )
        .await
        .unwrap();

        let messages = recent_messages.messages();
        let [context, first, second, rest, ..] = messages else {
            panic!("expected context, two messages and the rest, got {messages:?}");
        };
        assert_eq!(context.content, "[Resumed session] previous run");
        assert_eq!(context.role, residuum_inference::Role::User);
        assert!(context.context, "the resume pointer is marked as context");
        assert_eq!(context.id, None);
        assert!(
            !first.context && !second.context && !rest.context,
            "the messages a resume delivers are not context"
        );
        assert_eq!(first.id.as_deref(), Some("m-owner"));
        assert_eq!(first.content, owner.format_for_agent());
        assert_eq!(first.agent_sender, None, "the owner is not an agent");
        assert_eq!(second.id.as_deref(), Some("m-teammate"));
        assert_eq!(
            second.agent_sender.as_ref().map(|s| s.address.as_str()),
            Some("agent:ivan")
        );
        assert_eq!(rest.content, "[From: Jane via discord (#builds)] any news?");
        assert_eq!(rest.id, None);
    }

    #[tokio::test]
    async fn a_resume_by_chat_messages_alone_opens_with_its_pointer_as_its_own_entry() {
        let resources = make_resources("ack");
        let mut recent_messages = RecentMessages::new();
        let mut interrupt_rx = dead_interrupt_rx();

        execute_subagent(
            &test_identity("run-resume-chat"),
            TurnKickoff::Initial {
                prompt: "[From: Jane via discord (#builds)] any news?".to_string(),
                context: Some("[Resumed session] previous run".to_string()),
                hop_count: 0,
                sender: None,
                images: Vec::new(),
                delivered: Vec::new(),
                resumed_by_chat: true,
            },
            &mut recent_messages,
            &resources,
            TurnExecution {
                stop_token: &CancellationToken::new(),
                transcript_sink: None,
                usage_sink: None,
                interrupt_rx: &mut interrupt_rx,
            },
            None,
        )
        .await
        .unwrap();

        let messages = recent_messages.messages();
        let [context, chat, reply] = messages else {
            panic!("expected context, the chat messages and the reply, got {messages:?}");
        };
        assert_eq!(context.role, residuum_inference::Role::User);
        assert_eq!(context.content, "[Resumed session] previous run");
        assert!(context.context, "the resume pointer is marked as context");
        assert_eq!(chat.role, residuum_inference::Role::User);
        assert_eq!(chat.content, "[From: Jane via discord (#builds)] any news?");
        assert!(!chat.context, "the chat messages are not context");
        assert_eq!(reply.role, residuum_inference::Role::Assistant);
    }

    #[tokio::test]
    async fn external_kickoff_carries_sender_and_buffered_context() {
        let resources = make_resources("ack");
        let mut recent_messages = RecentMessages::new();
        let mut interrupt_rx = dead_interrupt_rx();

        let inbound = MessageEvent {
            id: "m1".to_string(),
            content: "can you check the build?".to_string(),
            origin: residuum_bus::origin::MessageOrigin {
                endpoint: "discord".to_string(),
                sender: Some(sample_sender()),
                conversation: Some(residuum_bus::origin::ConversationContext {
                    id: "chan-1".to_string(),
                    kind: residuum_bus::origin::ConversationKind::Channel,
                    is_owner: false,
                }),
                agent_sender: None,
            },
            timestamp: chrono::Utc::now().naive_utc(),
            images: vec![],
            attachments: Vec::new(),
            context: Some("[14:00] Sam: build is red".to_string()),
        };

        execute_subagent(
            &test_identity("run-external"),
            TurnKickoff::External(inbound),
            &mut recent_messages,
            &resources,
            TurnExecution {
                stop_token: &CancellationToken::new(),
                transcript_sink: None,
                usage_sink: None,
                interrupt_rx: &mut interrupt_rx,
            },
            None,
        )
        .await
        .unwrap();

        let messages = recent_messages.messages();
        let [context, user, ..] = messages else {
            panic!("expected context then user message, got {messages:?}");
        };
        assert_eq!(context.role, residuum_inference::Role::System);
        assert_eq!(context.content, "[14:00] Sam: build is red");
        assert_eq!(user.content, "can you check the build?");
        assert_eq!(user.sender.as_ref().map(|s| s.name.as_str()), Some("Jane"));
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

        let (tx, mut rx) = mpsc::unbounded_channel::<Interrupt>();
        tx.send(Interrupt::AgentMessage(AgentMessageEvent::new(
            residuum_bus::SessionAddress::from("main"),
            "main",
            "any updates?",
            0,
        )))
        .unwrap();

        let mut recent_messages = RecentMessages::new();
        let summary = execute_subagent(
            &test_identity("run-interrupt"),
            initial("keep working", None),
            &mut recent_messages,
            &resources,
            TurnExecution {
                stop_token: &CancellationToken::new(),
                transcript_sink: None,
                usage_sink: None,
                interrupt_rx: &mut rx,
            },
            None,
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

    #[tokio::test]
    async fn queued_conversation_message_is_drained_within_the_same_running_turn() {
        // Mirrors `queued_agent_message_is_drained_within_the_same_running_turn`
        // for `Interrupt::UserMessage`: a conversation session's running turn
        // must pick up a new message from its own conversation the same way,
        // via the shared `execute_turn` interrupt draining.
        let resources = make_resources("wrapping up");

        let (tx, mut rx) = mpsc::unbounded_channel::<Interrupt>();
        tx.send(Interrupt::UserMessage(MessageEvent {
            id: "m2".to_string(),
            content: "any updates?".to_string(),
            origin: residuum_bus::origin::MessageOrigin {
                endpoint: "discord".to_string(),
                sender: Some(sample_sender()),
                conversation: Some(residuum_bus::origin::ConversationContext {
                    id: "chan-1".to_string(),
                    kind: residuum_bus::origin::ConversationKind::Channel,
                    is_owner: false,
                }),
                agent_sender: None,
            },
            timestamp: chrono::Utc::now().naive_utc(),
            images: vec![],
            attachments: Vec::new(),
            context: None,
        }))
        .unwrap();

        let mut recent_messages = RecentMessages::new();
        let summary = execute_subagent(
            &test_identity("run-conv-interrupt"),
            initial("keep working", None),
            &mut recent_messages,
            &resources,
            TurnExecution {
                stop_token: &CancellationToken::new(),
                transcript_sink: None,
                usage_sink: None,
                interrupt_rx: &mut rx,
            },
            None,
        )
        .await
        .unwrap();

        assert_eq!(summary, "wrapping up");
        let messages = recent_messages.messages();
        assert!(
            messages.iter().any(|m| m.content == "any updates?"
                && m.sender.as_ref().map(|s| s.name.as_str()) == Some("Jane")),
            "the queued conversation message should be injected into the running turn \
             with its sender attribution intact, got {messages:?}"
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
            max_tool_iterations: None,
            repeat_call_guard: residuum_config::RepeatCallGuardConfig::default(),
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
            hop_counter: residuum_agent::HopCounter::new(0),
        };

        let mut recent_messages = RecentMessages::new();
        let mut interrupt_rx = dead_interrupt_rx();
        execute_subagent(
            &test_identity("run-fork"),
            initial("continue the task", None),
            &mut recent_messages,
            &resources,
            TurnExecution {
                stop_token: &CancellationToken::new(),
                transcript_sink: None,
                usage_sink: None,
                interrupt_rx: &mut interrupt_rx,
            },
            None,
        )
        .await
        .unwrap();

        let calls = seen.lock().unwrap();
        let call = calls.first().expect("provider should have been called");
        let system = call
            .iter()
            .find(|m| m.role == residuum_inference::Role::System)
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
            .find(|m| m.role == residuum_inference::Role::User)
            .expect("a user message must be present");
        assert_eq!(user.content, "continue the task");
        assert!(
            !user.content.contains("I am the agent."),
            "identity must not be duplicated into the user message"
        );
    }

    /// Returns a tool call plus text on its first call (so `execute_turn`
    /// takes the intermediate-publish branch and executes a tool), then
    /// plain text with no tool calls on every call after (ending the turn).
    struct ToolCallThenTextProvider {
        call_count: std::sync::atomic::AtomicUsize,
    }

    #[async_trait]
    impl InferenceProvider for ToolCallThenTextProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _options: &CompletionOptions,
        ) -> Result<InferenceResponse, InferenceError> {
            let n = self
                .call_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n == 0 {
                Ok(InferenceResponse::new(
                    "checking the build now".to_string(),
                    vec![residuum_inference::ToolCall {
                        id: "call-1".to_string(),
                        name: "nonexistent_tool".to_string(),
                        arguments: serde_json::json!({}),
                        server: None,
                    }],
                ))
            } else {
                Ok(InferenceResponse::new("build is green".to_string(), vec![]))
            }
        }

        fn model_name(&self) -> &'static str {
            "tool-call-then-text"
        }
    }

    #[tokio::test]
    async fn a_conversation_sessions_intermediate_text_reaches_its_own_conversation() {
        // Regression test: a conversation session's pre-tool-call text used
        // to go nowhere (sessions always ran with a noop publisher and no
        // output endpoint). With `conversation_output` set, it must reach
        // the session's own conversation as a `SessionResponseEvent` — the
        // same event and delivery path its final turn output uses — not
        // main's `IntermediateEvent`.
        let skill_state = SkillState::new_shared(SkillIndex::default(), vec![]);
        let mcp_registry = McpRegistry::new_shared();
        let (layout, observer, merge_writer) = test_memory_extras();
        let resources = SubAgentResources {
            max_tool_iterations: None,
            repeat_call_guard: residuum_config::RepeatCallGuardConfig::default(),
            provider: Box::new(ToolCallThenTextProvider {
                call_count: std::sync::atomic::AtomicUsize::new(0),
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
            hop_counter: residuum_agent::HopCounter::new(0),
        };

        let bus_handle = residuum_bus::spawn_broker();
        let publisher = bus_handle.publisher();
        let mut conv_sub: residuum_bus::Subscriber<residuum_bus::SessionResponseEvent> = bus_handle
            .subscribe(residuum_bus::topics::Endpoint(
                residuum_bus::EndpointName::from("discord"),
            ))
            .await
            .unwrap();
        let mut session_sub: residuum_bus::Subscriber<residuum_bus::SessionEvent> = bus_handle
            .subscribe(residuum_bus::topics::Sessions)
            .await
            .unwrap();

        let address = residuum_bus::SessionAddress::from("external-discord-chan-1");
        let identity = SessionTurnIdentity {
            publisher: &publisher,
            address: &address,
            run_id: "run-conv-tool",
            turn_id: "run-t1",
        };
        let mut recent_messages = RecentMessages::new();
        let mut interrupt_rx = dead_interrupt_rx();

        let summary = execute_subagent(
            &identity,
            initial("can you check the build?", None),
            &mut recent_messages,
            &resources,
            TurnExecution {
                stop_token: &CancellationToken::new(),
                transcript_sink: None,
                usage_sink: None,
                interrupt_rx: &mut interrupt_rx,
            },
            Some(ConversationOutput {
                endpoint: "discord",
                conversation_id: "chan-1",
            }),
        )
        .await
        .unwrap();

        assert_eq!(summary, "build is green");

        let event = next_event(
            "the intermediate text to reach the conversation",
            &mut conv_sub,
        )
        .await;
        assert_eq!(event.content, "checking the build now");
        assert_eq!(event.session_address, address);
        assert_eq!(event.conversation_id, "chan-1");

        let session_event =
            next_event("the session-stream event to be published", &mut session_sub).await;
        assert_eq!(session_event.address, address);
        assert_eq!(session_event.run_id, "run-conv-tool");
        assert!(
            matches!(
                session_event.kind,
                residuum_bus::SessionEventKind::Intermediate { ref content }
                    if content == "checking the build now"
            ),
            "a conversation session's intermediate text must reach the web sessions stream too"
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
            max_tool_iterations: None,
            repeat_call_guard: residuum_config::RepeatCallGuardConfig::default(),
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
            hop_counter: residuum_agent::HopCounter::new(0),
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
            TurnExecution {
                stop_token: &stop_token,
                transcript_sink: None,
                usage_sink: None,
                interrupt_rx: &mut interrupt_rx,
            },
            None,
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
            max_tool_iterations: None,
            repeat_call_guard: residuum_config::RepeatCallGuardConfig::default(),
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
            hop_counter: residuum_agent::HopCounter::new(0),
        };
        let store_dir = tempfile::tempdir().unwrap();
        let store = crate::store::SessionStore::new(store_dir.path().to_path_buf());
        let started_at = chrono::Utc::now();
        let sink = crate::store::RunTranscriptSink {
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
            TurnExecution {
                stop_token: &stop_token,
                transcript_sink: Some(&sink),
                usage_sink: None,
                interrupt_rx: &mut interrupt_rx,
            },
            None,
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
