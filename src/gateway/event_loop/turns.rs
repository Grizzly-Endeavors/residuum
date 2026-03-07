//! Agent turn handling and message processing in the event loop.

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::agent::context::{ProjectsContext, PromptContext, SkillsContext, SubagentsContext};
use crate::agent::interrupt::Interrupt;
use crate::gateway::types::{GatewayExit, GatewayRuntime};
use crate::interfaces::types::RoutedMessage;
use crate::memory::types::Visibility;
use crate::projects::activation::SharedProjectState;
use crate::skills::SharedSkillState;
use crate::workspace::layout::WorkspaceLayout;

use crate::agent::context::loading::{
    build_project_context_strings, build_skill_context_strings, build_subagents_context_string,
};
use super::background::{BackgroundContext, handle_background_result};
use crate::gateway::memory::{MemorySubsystems, execute_observation};

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
/// Background results are routed and observed; user messages are injected into
/// the agent's conversation. Returns `true` if any leftover triggered a wake request
/// (callers decide whether to act on it).
pub async fn process_leftover_interrupts(
    leftovers: Vec<Interrupt>,
    rt: &mut GatewayRuntime,
    observe_deadline: &mut Option<tokio::time::Instant>,
) -> bool {
    let mut wake = false;
    for intr in leftovers {
        match intr {
            Interrupt::BackgroundResult(bg_leftover) => {
                let bg_ctx = BackgroundContext {
                    router: &rt.notification_router,
                    layout: &rt.layout,
                    observer: &rt.observer,
                    project_state: &rt.project_state,
                    tz: rt.tz,
                };
                let bg_outcome =
                    handle_background_result(bg_leftover, &bg_ctx, &mut rt.agent, observe_deadline)
                        .await;
                if bg_outcome.force_observe {
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
                if bg_outcome.wake_requested {
                    wake = true;
                }
            }
            Interrupt::UserMessage(leftover_msg) => {
                rt.agent.inject_user_message(leftover_msg.content);
            }
        }
    }
    wake
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
    use crate::gateway::memory::{persist_and_check_thresholds, execute_observation};

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

/// Run an autonomous agent wake turn triggered by a background result.
///
/// Follows the same pattern as the inbound message handler but does not push
/// a user message — uses `run_wake_turn` which injects a system kickoff.
/// Broadcasts responses with `reply_to: "wake"` and persists messages with
/// `Visibility::Background`.
///
/// Returns `Some(GatewayExit)` if a reload signal fires during the turn.
pub async fn run_wake_turn_handler(
    rt: &mut GatewayRuntime,
    observe_deadline: &mut Option<tokio::time::Instant>,
) -> Option<GatewayExit> {
    use crate::gateway::protocol::ServerMessage;

    let Some(reply) = rt.last_reply.as_ref().map(Arc::clone) else {
        tracing::warn!("wake turn requested but no channel has connected yet, skipping");
        return None;
    };

    tracing::info!("starting autonomous wake turn from background result");

    let before = rt.agent.message_count();

    let ctx_strings =
        load_prompt_context_strings(&rt.project_state, &rt.skill_state, &rt.layout).await;
    let prompt_ctx = ctx_strings.as_prompt_context();

    let typing_guard = reply.start_typing();
    let (interrupt_tx, mut interrupt_rx) = mpsc::channel::<Interrupt>(32);

    let turn_result = {
        let mut turn = std::pin::pin!(rt.agent.run_wake_turn(
            &*reply,
            &prompt_ctx,
            &mut interrupt_rx,
        ));

        loop {
            tokio::select! {
                result = &mut turn => break result,
                next_msg = rt.inbound_rx.recv() => {
                    if let Some(next_routed) = next_msg {
                        drop(interrupt_tx.try_send(
                            Interrupt::UserMessage(next_routed.message)
                        ));
                    }
                }
                bg_result = rt.background_result_rx.recv() => {
                    if let Some(result) = bg_result {
                        drop(interrupt_tx.try_send(
                            Interrupt::BackgroundResult(result)
                        ));
                    }
                }
                _ = rt.reload_rx.changed() => {
                    tracing::info!("reload signal received during wake turn, deferring");
                }
            }
        }
    };

    drop(interrupt_tx);
    let leftover_interrupts = drain_interrupts(&mut interrupt_rx);

    match turn_result {
        Ok(texts) => {
            drop(typing_guard);
            for text in &texts {
                reply.send_response(text).await;
            }
        }
        Err(e) => {
            drop(typing_guard);
            tracing::warn!(error = %e, "wake turn processing error");
            if rt
                .broadcast_tx
                .send(ServerMessage::Error {
                    reply_to: Some("wake".to_string()),
                    message: e.to_string(),
                })
                .is_err()
            {
                tracing::trace!("no broadcast receivers for wake error");
            }
        }
    }

    let new_messages: Vec<_> = rt.agent.messages_since(before).to_vec();
    persist_and_maybe_observe(rt, &new_messages, Visibility::Background, observe_deadline).await;

    // Don't recursively trigger wake turns from leftovers
    process_leftover_interrupts(leftover_interrupts, rt, observe_deadline).await;

    None
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

    // TurnStarted is WS-specific protocol sugar
    if origin.interface == "websocket" {
        rt.broadcast_tx
            .send(ServerMessage::TurnStarted {
                reply_to: reply_id.clone(),
            })
            .ok();
    }

    rt.last_reply = Some(Arc::clone(&routed.reply));
    if let std::collections::hash_map::Entry::Vacant(e) =
        rt.unsolicited_handles.entry(origin.interface.clone())
        && let Some(h) = routed.reply.unsolicited_clone()
    {
        e.insert(h);
    }
    let typing_guard = routed.reply.start_typing();
    let before = rt.agent.message_count();

    let ctx_strings =
        load_prompt_context_strings(&rt.project_state, &rt.skill_state, &rt.layout).await;
    let prompt_ctx = ctx_strings.as_prompt_context();

    let (interrupt_tx, mut interrupt_rx) = mpsc::channel::<Interrupt>(32);
    let turn_result = {
        let mut turn = std::pin::pin!(rt.agent.process_message(
            &routed.message.content,
            &*routed.reply,
            Some(&origin),
            &prompt_ctx,
            &mut interrupt_rx,
            &routed.message.images,
        ));
        loop {
            tokio::select! {
                result = &mut turn => break result,
                next_msg = rt.inbound_rx.recv() => {
                    if let Some(next_routed) = next_msg {
                        drop(interrupt_tx.try_send(Interrupt::UserMessage(next_routed.message)));
                    }
                }
                bg_result = rt.background_result_rx.recv() => {
                    if let Some(result) = bg_result {
                        drop(interrupt_tx.try_send(Interrupt::BackgroundResult(result)));
                    }
                }
                _ = rt.reload_rx.changed() => {
                    tracing::info!("reload signal received during active turn, deferring");
                }
            }
        }
    };

    drop(interrupt_tx);
    let leftover_interrupts = drain_interrupts(&mut interrupt_rx);

    drop(typing_guard);
    match turn_result {
        Ok(texts) => {
            for text in &texts {
                routed.reply.send_response(text).await;
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "agent processing error");
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

    if process_leftover_interrupts(leftover_interrupts, rt, observe_deadline).await
        && let Some(exit) = run_wake_turn_handler(rt, observe_deadline).await
    {
        return Some(exit);
    }

    if !rt.cfg.idle.timeout.is_zero() {
        let now = tokio::time::Instant::now();
        rt.last_user_message_instant = Some(now);
        *idle_deadline = Some(now + rt.cfg.idle.timeout);
    }
    None
}
