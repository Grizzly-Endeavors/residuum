//! Background task result handling and routing in the event loop.

use crate::agent::Agent;
use crate::background::types::{BackgroundResult, ResultRouting, format_background_result};
use crate::memory::observer::Observer;
use crate::memory::types::Visibility;
use crate::models::Message;
use crate::notify::router::NotificationRouter;
use crate::notify::types::{
    BuiltinChannel, ChannelTarget, Notification, TaskSource, parse_channel_list,
};
use crate::projects::activation::SharedProjectState;
use crate::workspace::layout::WorkspaceLayout;

use crate::gateway::helpers::project_context_label;
use crate::gateway::memory::persist_and_check_thresholds;

/// Outcome of handling a background task result.
pub struct BackgroundResultOutcome {
    /// Whether observation should fire immediately (token threshold exceeded).
    pub force_observe: bool,
    /// Whether the agent should start an autonomous wake turn.
    pub wake_requested: bool,
}

/// Bundled context for handling background task results.
pub struct BackgroundContext<'a> {
    pub router: &'a NotificationRouter,
    pub layout: &'a WorkspaceLayout,
    pub observer: &'a Observer,
    pub project_state: &'a SharedProjectState,
    pub tz: chrono_tz::Tz,
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

/// Handle a background task result: route to channels.
///
/// When a result targets the agent feed or wake, the formatted message is persisted to
/// `recent_messages.json` and injected into the agent's conversation history
/// immediately — no longer deferred to the next user turn.
pub(super) async fn handle_background_result(
    result: BackgroundResult,
    ctx: &BackgroundContext<'_>,
    agent: &mut Agent,
    observe_deadline: &mut Option<tokio::time::Instant>,
) -> BackgroundResultOutcome {
    let no_action = BackgroundResultOutcome {
        force_observe: false,
        wake_requested: false,
    };

    // Pulse HEARTBEAT_OK results are silently logged — no routing, no agent events
    if matches!(result.source, TaskSource::Pulse) && result.summary.contains("HEARTBEAT_OK") {
        tracing::info!(task = %result.task_name, "pulse check: HEARTBEAT_OK");
        return no_action;
    }

    let formatted = format_background_result(&result);

    let ResultRouting::Direct(channels) = &result.routing;
    let targets = parse_channel_list(channels);
    let (should_inject, wake) = {
        let mut agent_inject = false;
        let mut wake_requested = false;
        for target in &targets {
            match target {
                ChannelTarget::Builtin(BuiltinChannel::AgentWake) => {
                    agent_inject = true;
                    wake_requested = true;
                }
                ChannelTarget::Builtin(BuiltinChannel::AgentFeed) => agent_inject = true,
                ChannelTarget::Builtin(BuiltinChannel::Inbox) => {
                    let notification = Notification {
                        task_name: result.task_name.clone(),
                        summary: result.summary.clone(),
                        source: result.source,
                        timestamp: result.timestamp,
                    };
                    if let Err(e) = ctx.router.deliver_to_inbox(&notification).await {
                        tracing::warn!(
                            task = %result.task_name,
                            error = %e,
                            "inbox delivery failed"
                        );
                    }
                }
                ChannelTarget::External(ext_name) => {
                    tracing::warn!(
                        channel = %ext_name,
                        task = %result.task_name,
                        "direct channel routing (external channels not yet supported for direct)"
                    );
                }
            }
        }
        (agent_inject, wake_requested)
    };

    if !should_inject {
        return no_action;
    }

    // Persist immediately so the message survives restarts
    let sys_msg = Message::system(&formatted);
    let project_ctx = project_context_label(ctx.project_state, ctx.layout).await;
    let action = persist_and_check_thresholds(
        &[sys_msg],
        &project_ctx,
        Visibility::Background,
        ctx.observer,
        ctx.layout,
        ctx.tz,
    )
    .await;

    let force = apply_observe_action(action, observe_deadline, ctx.observer.cooldown_secs());

    // Inject into LLM context
    agent.inject_system_message(formatted);

    BackgroundResultOutcome {
        force_observe: force,
        wake_requested: wake,
    }
}
