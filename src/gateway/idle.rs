//! Idle transition logic.
//!
//! After a configurable period of user inactivity the gateway deactivates
//! active skills, fires the observer, clears the message buffer, and
//! injects a continuity system message. The observer fire-and-clear-and-summarize
//! sequence runs through the same background worker as every other automatic
//! observe (see `crate::gateway::post_turn`), so [`execute_idle_transition`]
//! itself returns almost immediately — [`apply_idle_continuation`] finishes
//! the sequence once that background cycle's result comes back on the main
//! loop.

use std::sync::Arc;

use crate::gateway::memory::MemorySubsystems;
use crate::gateway::post_turn::IdleContinuation;
use crate::gateway::types::GatewayRuntime;

/// Run the idle transition's synchronous steps and trigger its observe
/// cycle in the background; [`apply_idle_continuation`] finishes the rest
/// once that cycle's result comes back.
#[tracing::instrument(skip_all)]
pub(super) async fn execute_idle_transition(
    rt: &mut GatewayRuntime,
    observe_deadline: &mut Option<tokio::time::Instant>,
) {
    let timeout_mins = rt.cfg.idle.timeout.as_secs() / 60;
    tracing::info!(timeout_mins, "idle timeout reached, transitioning");

    // 1. Deactivate explicitly-activated skills
    let total_skills = deactivate_remaining_skills(rt).await;
    *observe_deadline = None;

    // 2. Trigger the observe cycle in the background; steps 3-5 (clearing
    // messages, switching the interface, injecting the continuity note)
    // wait for it via `apply_idle_continuation` so a message that hasn't
    // yet been observed is never wiped out by the clear below racing ahead
    // of it.
    let mem = MemorySubsystems {
        observer: Arc::clone(&rt.observer),
        merge_writer: Arc::clone(&rt.merge_writer),
        layout: rt.layout.clone(),
        tz: rt.tz,
        publisher: rt.publisher.clone(),
    };
    rt.post_turn_observe.trigger_for_idle(
        mem,
        IdleContinuation {
            timeout_mins,
            total_skills,
            idle_channel: rt.cfg.idle.idle_channel.clone(),
        },
    );
}

/// Finish an idle transition once its background observe cycle's result
/// arrives: clear the in-memory message buffer, switch the notification
/// interface, and inject the continuity system message. Called from
/// `run_loop`'s `apply_post_turn_result`.
pub(super) fn apply_idle_continuation(rt: &mut GatewayRuntime, continuation: &IdleContinuation) {
    rt.agent.clear_messages();

    if let Some(channel_name) = &continuation.idle_channel {
        switch_idle_interface(rt, channel_name);
    }

    let summary = format_idle_summary(continuation.timeout_mins, continuation.total_skills);
    rt.agent.inject_system_message(&summary);
}

/// Switch `last_output_topic` to the configured idle channel.
///
/// Validates the endpoint exists in the registry and has interactive capability
/// before switching. Falls back to current topic if the endpoint is not found.
fn switch_idle_interface(rt: &mut GatewayRuntime, channel_name: &str) {
    let endpoint_id = crate::bus::EndpointId::from(channel_name);
    match rt.endpoint_registry.get(&endpoint_id) {
        Some(entry)
            if entry
                .capabilities
                .contains(crate::bus::EndpointCapabilities::INTERACTIVE) =>
        {
            rt.last_output_endpoint = Some(crate::bus::EndpointName::from(channel_name));
            tracing::info!(channel = %channel_name, "switched to idle interface");
        }
        Some(_) => {
            tracing::warn!(
                channel = %channel_name,
                "idle channel exists but is not interactive, keeping current output topic"
            );
        }
        None => {
            tracing::warn!(
                channel = %channel_name,
                "idle channel not found in endpoint registry, keeping current output topic"
            );
        }
    }
}

/// Deactivate all remaining explicitly-activated skills.
async fn deactivate_remaining_skills(rt: &mut GatewayRuntime) -> usize {
    let mut state = rt.skill_state.lock().await;
    let names: Vec<String> = state
        .active_skill_names()
        .into_iter()
        .map(String::from)
        .collect();
    let count = names.len();

    for name in &names {
        if let Err(e) = state.deactivate(name) {
            tracing::warn!(skill = %name, error = %e, "failed to deactivate skill during idle");
        }
    }

    if count > 0 {
        tracing::info!(count, "deactivated remaining skills during idle transition");
    }

    count
}

/// Build the idle summary message injected into the agent context.
fn format_idle_summary(timeout_mins: u64, skill_count: usize) -> String {
    let mut parts = vec![format!(
        "[Idle] Transitioned to idle after {timeout_mins}m of inactivity."
    )];

    if skill_count > 0 {
        parts.push(format!(
            "Deactivated {skill_count} skill{}.",
            if skill_count == 1 { "" } else { "s" }
        ));
    }

    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_idle_summary_with_skills() {
        let result = format_idle_summary(15, 3);
        assert_eq!(
            result,
            "[Idle] Transitioned to idle after 15m of inactivity. Deactivated 3 skills."
        );
    }

    #[test]
    fn format_idle_summary_nothing_active() {
        let result = format_idle_summary(30, 0);
        assert_eq!(
            result,
            "[Idle] Transitioned to idle after 30m of inactivity."
        );
    }

    #[test]
    fn format_idle_summary_single_skill_no_plural() {
        let result = format_idle_summary(30, 1);
        assert_eq!(
            result,
            "[Idle] Transitioned to idle after 30m of inactivity. Deactivated 1 skill."
        );
    }
}
