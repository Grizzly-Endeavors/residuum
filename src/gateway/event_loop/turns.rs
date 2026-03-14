//! Agent turn handling and message processing in the event loop.

use tokio::sync::mpsc;

use crate::agent::Agent;
use crate::agent::context::{ProjectsContext, PromptContext, SkillsContext, SubagentsContext};
use crate::agent::interrupt::Interrupt;
use crate::background::types::BackgroundResult;
use crate::bus::{BusEvent, EndpointName, Publisher, ResponseEvent, Subscriber, TopicId};
use crate::error::ResiduumError;
use crate::gateway::types::{GatewayExit, GatewayRuntime, ReloadSignal};
use crate::interfaces::types::{MessageOrigin, RoutedMessage};
use crate::memory::types::Visibility;
use crate::models::ImageData;
use crate::projects::activation::SharedProjectState;
use crate::skills::SharedSkillState;
use crate::workspace::layout::WorkspaceLayout;

use super::background::{BackgroundContext, handle_background_result};
use crate::agent::context::loading::{
    build_project_context_strings, build_skill_context_strings, build_subagents_context_string,
};
use crate::gateway::memory::MemorySubsystems;

/// Raw prompt context strings for constructing a `PromptContext`.
///
/// Held as owned `Option<String>` so that `PromptContext` can borrow via `as_deref()`.
pub struct PromptContextStrings {
    pub proj_index: Option<String>,
    pub proj_active: Option<String>,
    pub skill_index: Option<String>,
    pub skill_active: Option<String>,
    pub subagents_index: Option<String>,
}

impl PromptContextStrings {
    /// Build a borrowed `PromptContext` from these owned strings.
    pub(super) fn as_prompt_context(&self) -> PromptContext<'_> {
        PromptContext {
            projects: ProjectsContext {
                index: self.proj_index.as_deref(),
                active_context: self.proj_active.as_deref(),
            },
            skills: SkillsContext {
                index: self.skill_index.as_deref(),
                active_instructions: self.skill_active.as_deref(),
            },
            subagents: SubagentsContext {
                index: self.subagents_index.as_deref(),
            },
        }
    }
}

/// Load prompt context strings from project, skill, and subagent state.
pub async fn load_prompt_context_strings(
    project_state: &SharedProjectState,
    skill_state: &SharedSkillState,
    layout: &WorkspaceLayout,
) -> PromptContextStrings {
    let (proj_index, proj_active) = build_project_context_strings(project_state).await;
    let (skill_index, skill_active) = build_skill_context_strings(skill_state).await;
    let subagents_index = build_subagents_context_string(&layout.subagents_dir()).await;
    PromptContextStrings {
        proj_index,
        proj_active,
        skill_index,
        skill_active,
        subagents_index,
    }
}

/// Process leftover interrupts that arrived during an agent turn but weren't consumed.
///
/// Background results are published to notification topics; user messages are
/// injected into the agent's conversation.
pub async fn process_leftover_interrupts(
    leftovers: Vec<Interrupt>,
    rt: &mut GatewayRuntime,
    _observe_deadline: &mut Option<tokio::time::Instant>,
) {
    for intr in leftovers {
        match intr {
            Interrupt::BackgroundResult(bg_leftover) => {
                let bg_ctx = BackgroundContext {
                    publisher: &rt.publisher,
                    tz: rt.tz,
                };
                handle_background_result(bg_leftover, &bg_ctx).await;
            }
            Interrupt::UserMessage(leftover_msg) => {
                rt.agent.inject_user_message(leftover_msg.content);
            }
        }
    }
}

/// Drain remaining interrupts from an interrupt channel after a turn completes.
pub fn drain_interrupts(interrupt_rx: &mut mpsc::Receiver<Interrupt>) -> Vec<Interrupt> {
    let mut leftovers = Vec::new();
    while let Ok(intr) = interrupt_rx.try_recv() {
        leftovers.push(intr);
    }
    leftovers
}

/// Persist new messages and run observation if thresholds are exceeded.
pub async fn persist_and_maybe_observe(
    rt: &mut GatewayRuntime,
    new_messages: &[crate::models::Message],
    visibility: Visibility,
    observe_deadline: &mut Option<tokio::time::Instant>,
) {
    use crate::gateway::helpers::project_context_label;
    use crate::gateway::memory::{execute_observation, persist_and_check_thresholds};

    let project_ctx = project_context_label(&rt.project_state, &rt.layout).await;
    let action = persist_and_check_thresholds(
        new_messages,
        &project_ctx,
        visibility,
        &rt.observer,
        &rt.layout,
        rt.tz,
    )
    .await;
    if apply_observe_action(action, observe_deadline, rt.observer.cooldown_secs()) {
        let mem = MemorySubsystems {
            observer: &rt.observer,
            reflector: &rt.reflector,
            search_index: &rt.search_index,
            layout: &rt.layout,
            vector_store: rt.vector_store.as_ref(),
            embedding_provider: rt.embedding_provider.as_ref(),
        };
        execute_observation(&mem, &mut rt.agent).await;
    }
}

/// Helper to apply observe action and update deadline.
fn apply_observe_action(
    action: crate::memory::observer::ObserveAction,
    observe_deadline: &mut Option<tokio::time::Instant>,
    cooldown_secs: u64,
) -> bool {
    use crate::memory::observer::ObserveAction;
    match action {
        ObserveAction::None => false,
        ObserveAction::StartCooldown => {
            *observe_deadline =
                Some(tokio::time::Instant::now() + tokio::time::Duration::from_secs(cooldown_secs));
            false
        }
        ObserveAction::ForceNow => {
            *observe_deadline = None;
            true
        }
    }
}

/// Run an agent turn while monitoring interrupt sources (bus messages, background
/// results, reload signals). Returns the turn result and any leftover interrupts
/// that arrived after the turn completed.
#[expect(
    clippy::too_many_arguments,
    reason = "publisher and topic params added during bus migration"
)]
async fn run_agent_turn_with_interrupts(
    agent: &mut Agent,
    content: &str,
    publisher: &Publisher,
    output_topic: &TopicId,
    origin: Option<&MessageOrigin>,
    prompt_ctx: &PromptContext<'_>,
    images: &[ImageData],
    agent_subscriber: &mut Subscriber,
    background_result_rx: &mut mpsc::Receiver<BackgroundResult>,
    reload_rx: &mut tokio::sync::watch::Receiver<ReloadSignal>,
) -> (Result<Vec<String>, ResiduumError>, Vec<Interrupt>) {
    let (interrupt_tx, mut interrupt_rx) = mpsc::channel::<Interrupt>(32);
    let turn_result = {
        let mut turn = std::pin::pin!(agent.process_message(
            content,
            publisher,
            output_topic,
            origin,
            prompt_ctx,
            &mut interrupt_rx,
            images,
        ));
        loop {
            tokio::select! {
                result = &mut turn => break result,
                next_msg = agent_subscriber.recv() => {
                    if let Some(BusEvent::Message(msg_event)) = next_msg {
                        let inbound = crate::interfaces::types::InboundMessage {
                            id: msg_event.id,
                            content: msg_event.content,
                            origin: msg_event.origin,
                            timestamp: chrono::Utc::now(),
                            images: msg_event.images,
                        };
                        if interrupt_tx.try_send(Interrupt::UserMessage(inbound)).is_err() {
                            tracing::warn!("interrupt channel full, dropping user message mid-turn");
                        }
                    }
                }
                bg_result = background_result_rx.recv() => {
                    if let Some(result) = bg_result
                        && interrupt_tx.try_send(Interrupt::BackgroundResult(result)).is_err() {
                            tracing::warn!("interrupt channel full, dropping background result mid-turn");
                        }
                }
                _ = reload_rx.changed() => {
                    tracing::info!("reload signal received during active turn, deferring");
                }
            }
        }
    };

    drop(interrupt_tx);
    let leftover_interrupts = drain_interrupts(&mut interrupt_rx);

    (turn_result, leftover_interrupts)
}

/// Handle an inbound user message: run agent turn, persist, observe, and process leftovers.
///
/// Returns `Some(GatewayExit)` if a shutdown-worthy event occurs during processing.
pub async fn handle_inbound_message(
    routed: RoutedMessage,
    rt: &mut GatewayRuntime,
    observe_deadline: &mut Option<tokio::time::Instant>,
    idle_deadline: &mut Option<tokio::time::Instant>,
) -> Option<GatewayExit> {
    use crate::gateway::protocol::ServerMessage;

    let reply_id = routed.message.id.clone();
    let origin = routed.message.origin.clone();

    let output_topic = TopicId::Interactive(EndpointName::from(origin.endpoint.as_str()));
    rt.last_output_topic = Some(output_topic.clone());

    drop(
        rt.publisher
            .publish(
                output_topic.clone(),
                BusEvent::TurnStarted {
                    correlation_id: reply_id.clone(),
                },
            )
            .await,
    );

    let before = rt.agent.message_count();

    let ctx_strings =
        load_prompt_context_strings(&rt.project_state, &rt.skill_state, &rt.layout).await;
    let prompt_ctx = ctx_strings.as_prompt_context();

    let (turn_result, leftover_interrupts) = run_agent_turn_with_interrupts(
        &mut rt.agent,
        &routed.message.content,
        &rt.publisher,
        &output_topic,
        Some(&origin),
        &prompt_ctx,
        &routed.message.images,
        &mut rt.agent_subscriber,
        &mut rt.background_result_rx,
        &mut rt.reload_rx,
    )
    .await;

    match turn_result {
        Ok(texts) => {
            for text in &texts {
                drop(
                    rt.publisher
                        .publish(
                            output_topic.clone(),
                            BusEvent::Response(ResponseEvent {
                                correlation_id: reply_id.clone(),
                                content: text.clone(),
                                timestamp: crate::time::now_local(rt.tz),
                            }),
                        )
                        .await,
                );
            }
            drop(
                rt.publisher
                    .publish(
                        output_topic.clone(),
                        BusEvent::TurnEnded {
                            correlation_id: reply_id.clone(),
                        },
                    )
                    .await,
            );
        }
        Err(e) => {
            tracing::warn!(error = %e, "agent processing error");
            drop(
                rt.publisher
                    .publish(
                        output_topic.clone(),
                        BusEvent::TurnEnded {
                            correlation_id: reply_id.clone(),
                        },
                    )
                    .await,
            );
            rt.broadcast_tx
                .send(ServerMessage::Error {
                    reply_to: Some(reply_id),
                    message: e.to_string(),
                })
                .ok();
        }
    }

    let new_messages: Vec<_> = rt.agent.messages_since(before).to_vec();
    persist_and_maybe_observe(rt, &new_messages, Visibility::User, observe_deadline).await;

    process_leftover_interrupts(leftover_interrupts, rt, observe_deadline).await;

    if !rt.cfg.idle.timeout.is_zero() {
        let now = tokio::time::Instant::now();
        rt.last_user_message_instant = Some(now);
        *idle_deadline = Some(now + rt.cfg.idle.timeout);
    }
    None
}
