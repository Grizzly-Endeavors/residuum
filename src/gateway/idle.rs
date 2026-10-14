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

use crate::bus::EndpointName;
use crate::gateway::memory::MemorySubsystems;
use crate::gateway::post_turn::IdleContinuation;
use crate::gateway::types::AgentRuntime;
use crate::interfaces::types::WEB_UI_ENDPOINT;

/// Run the idle transition's synchronous steps and trigger its observe
/// cycle in the background; [`apply_idle_continuation`] finishes the rest
/// once that cycle's result comes back.
#[tracing::instrument(skip_all)]
pub(super) async fn execute_idle_transition(
    rt: &mut AgentRuntime,
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
    let mem = MemorySubsystems::from_runtime(rt);
    rt.memory_worker.trigger_for_idle(
        mem,
        IdleContinuation {
            timeout_mins,
            total_skills,
            idle_channel: rt.cfg.idle.idle_channel.clone(),
        },
    );
}

/// Finish an idle transition once its background observe cycle's result
/// arrives: clear the in-memory message buffer, switch where agent-started
/// turns go, and inject the continuity system message. Called from
/// `run_loop`'s `apply_post_turn_result`.
pub(super) fn apply_idle_continuation(rt: &mut AgentRuntime, continuation: &IdleContinuation) {
    rt.agent.clear_messages();

    rt.last_output_endpoint = Some(idle_output_endpoint(
        &rt.endpoint_registry,
        &continuation.idle_channel,
    ));

    let summary = format_idle_summary(continuation.timeout_mins, continuation.total_skills);
    rt.agent.inject_system_message(&summary);
}

/// Where agent-started main turns go once the owner is idle: the configured
/// idle channel when the registry has it as an interactive endpoint, else
/// the web UI, which is always registered. Falling back to the web rather
/// than leaving output where the owner last spoke keeps it off a chat the
/// owner did not choose for this.
pub(super) fn idle_output_endpoint(
    registry: &crate::bus::EndpointRegistry,
    channel_name: &str,
) -> EndpointName {
    let web = || EndpointName::from(WEB_UI_ENDPOINT);
    match registry.get(&crate::bus::EndpointId::from(channel_name)) {
        Some(entry)
            if entry
                .capabilities
                .contains(crate::bus::EndpointCapabilities::INTERACTIVE) =>
        {
            tracing::info!(channel = %channel_name, "switched to idle interface");
            EndpointName::from(channel_name)
        }
        Some(_) => {
            tracing::warn!(
                channel = %channel_name,
                "idle channel exists but is not interactive, sending idle output to the web UI"
            );
            web()
        }
        None => {
            tracing::warn!(
                channel = %channel_name,
                "idle channel not found in endpoint registry, sending idle output to the web UI"
            );
            web()
        }
    }
}

/// Deactivate all remaining explicitly-activated skills.
async fn deactivate_remaining_skills(rt: &mut AgentRuntime) -> usize {
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
    use crate::bus::{EndpointCapabilities, EndpointEntry, EndpointId, EndpointRegistry, TopicId};

    fn entry(id: &str, capabilities: EndpointCapabilities) -> EndpointEntry {
        EndpointEntry {
            id: EndpointId::from(id),
            topic: TopicId::Endpoint(EndpointName::from(id)),
            capabilities,
            display_name: id.to_string(),
        }
    }

    fn registry_with_telegram() -> EndpointRegistry {
        EndpointRegistry::from_entries([
            entry(WEB_UI_ENDPOINT, EndpointCapabilities::INTERACTIVE),
            entry("telegram", EndpointCapabilities::INTERACTIVE),
            entry("ntfy", EndpointCapabilities::empty()),
        ])
    }

    #[test]
    fn idle_output_goes_to_the_configured_interactive_channel() {
        let registry = registry_with_telegram();
        assert_eq!(
            idle_output_endpoint(&registry, "telegram"),
            EndpointName::from("telegram")
        );
        assert_eq!(
            idle_output_endpoint(&registry, WEB_UI_ENDPOINT),
            EndpointName::from(WEB_UI_ENDPOINT)
        );
    }

    #[test]
    fn idle_output_falls_back_to_the_web_ui_for_a_missing_or_non_interactive_channel() {
        let registry = registry_with_telegram();
        for channel in ["discord", "ntfy"] {
            assert_eq!(
                idle_output_endpoint(&registry, channel),
                EndpointName::from(WEB_UI_ENDPOINT),
                "{channel} must not leave idle output on a chat"
            );
        }
    }

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
