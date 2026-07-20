//! End-of-turn subconscious evaluation and finding delivery.
//!
//! Runs after a user-visible turn completes. It is a triage step, not a blind
//! re-classification: it receives the steering the mid-turn watch already
//! applied (via `TurnScratch`) so it can avoid repeating corrections and fold
//! in queued notes. `note` findings are injected as passive context for the
//! next turn; the first `act` finding triggers an immediate correction turn by
//! publishing a background-origin `MessageEvent` — the same mechanism the
//! notification router uses to wake the main agent. Background turns are never
//! evaluated, which is what prevents a correction turn from triggering another
//! evaluation.
//!
//! This runs synchronously on the gateway event loop (mirroring the observer),
//! so it adds one classifier round-trip before the next inbound message is
//! handled. The user-visible reply has already been sent by this point.

use std::sync::{Arc, Mutex};

use crate::bus::topics;
use crate::gateway::types::GatewayRuntime;
use crate::models::Message;
use crate::subconscious::{EvalPhase, Finding, LearnSignal, Severity, TurnScratch};

/// A planned delivery for one finding, decided before touching the runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Delivery {
    /// Inject as a passive system note for the next turn.
    Note(String),
    /// Trigger an immediate correction turn.
    Correction(String),
}

/// Evaluate a completed turn and deliver any findings to the agent.
///
/// Never fails the turn: evaluation errors are logged and swallowed.
pub(super) async fn run_end_of_turn_subconscious(
    rt: &mut GatewayRuntime,
    new_messages: &[Message],
    correlation_id: &str,
    scratch: Option<&Arc<Mutex<TurnScratch>>>,
) {
    if !rt.subconscious.enabled() {
        return;
    }
    // A turn with no assistant output (e.g. hard error) has nothing to judge.
    if new_messages.len() < 2 {
        return;
    }

    // Snapshot the mid-turn scratch so the classifier can triage against what
    // already happened this turn. A poisoned lock degrades to no prior context.
    let prior = scratch
        .and_then(|s| s.lock().ok().map(|guard| guard.clone()))
        .unwrap_or_default();

    let subconscious = Arc::clone(&rt.subconscious);
    match subconscious
        .evaluate(new_messages, EvalPhase::EndOfTurn, Some(&prior))
        .await
    {
        Ok(outcome) => {
            for delivery in plan_delivery(outcome.findings) {
                match delivery {
                    Delivery::Note(instruction) => {
                        tracing::info!("subconscious note queued for next turn");
                        rt.agent
                            .inject_system_message(format!("[Subconscious note] {instruction}"));
                    }
                    Delivery::Correction(instruction) => {
                        tracing::info!("subconscious act finding triggering correction turn");
                        publish_correction_turn(rt, &instruction, correlation_id).await;
                    }
                }
            }
            // Learnable signals feed the learner sub-agent (cooldown-gated).
            maybe_spawn_learner(rt, &outcome.learnings).await;
        }
        Err(e) => {
            tracing::warn!(error = %e, "subconscious end-of-turn evaluation failed");
            // Triage is the only delivery path for queued mid-turn notes, so a
            // failed evaluation would silently drop them. Surface them raw
            // instead of losing them.
            deliver_fallback_notes(rt, &prior);
        }
    }
}

/// Spawn the `learner` sub-agent when the triage surfaced learnable signals.
///
/// Cooldown-gated via the runtime's `LearningState`; an empty signal list or a
/// live cooldown is a no-op. Decisions are logged inside `LearningState`.
async fn maybe_spawn_learner(rt: &mut GatewayRuntime, learnings: &[LearnSignal]) {
    let cooldown = rt.cfg.subconscious_settings.learning_cooldown();
    let Some(spawn) =
        rt.learning_state
            .on_learn_signals(learnings, cooldown, std::time::Instant::now())
    else {
        return;
    };
    if let Err(e) = rt.publisher.publish(topics::Background, spawn).await {
        tracing::warn!(error = %e, "failed to publish learner spawn request");
    } else {
        tracing::info!(
            signals = learnings.len(),
            "learner sub-agent spawn requested"
        );
    }
}

/// Deliver queued mid-turn notes directly when the triage evaluation failed.
///
/// These are the lower-urgency findings the mid-turn watch observed but did not
/// steer on; without the triage pass to fold them in, they are injected as-is
/// so the agent still sees them next turn.
fn deliver_fallback_notes(rt: &mut GatewayRuntime, prior: &TurnScratch) {
    if prior.queued_notes.is_empty() {
        return;
    }
    tracing::info!(
        count = prior.queued_notes.len(),
        "delivering queued mid-turn notes after triage failure"
    );
    for note in &prior.queued_notes {
        rt.agent
            .inject_system_message(format!("[Subconscious note] {}", note.instruction));
    }
}

/// Decide how each finding is delivered.
///
/// The first `act` finding becomes a correction turn; every other finding
/// (including later `act` findings) degrades to a note, so a single turn never
/// spawns more than one correction.
fn plan_delivery(findings: Vec<Finding>) -> Vec<Delivery> {
    let mut plan = Vec::with_capacity(findings.len());
    let mut correction_used = false;
    for finding in findings {
        if finding.severity == Severity::Act && !correction_used {
            correction_used = true;
            plan.push(Delivery::Correction(finding.instruction));
        } else {
            plan.push(Delivery::Note(finding.instruction));
        }
    }
    plan
}

/// Build the background-origin `MessageEvent` that starts a correction turn.
///
/// The `background` origin endpoint is what keeps the correction turn from
/// being re-evaluated by the subconscious (the end-of-turn hook skips
/// background turns), bounding the feedback loop.
fn build_correction_event(
    instruction: &str,
    correlation_id: &str,
    timestamp: chrono::NaiveDateTime,
) -> crate::bus::MessageEvent {
    let content = format!(
        "[Subconscious] A background check of your last turn found a problem to correct now:\n\
         {instruction}\n\
         Take the corrective action (e.g. update MEMORY.md). Only message the user if they \
         need to know something new."
    );

    crate::bus::MessageEvent {
        id: format!("subconscious-{correlation_id}"),
        content,
        origin: crate::interfaces::types::MessageOrigin {
            endpoint: "background".to_string(),
            sender_name: "subconscious".to_string(),
            sender_id: correlation_id.to_string(),
        },
        timestamp,
        images: vec![],
    }
}

/// Publish a correction turn onto the user-message topic.
async fn publish_correction_turn(rt: &GatewayRuntime, instruction: &str, correlation_id: &str) {
    let msg_event =
        build_correction_event(instruction, correlation_id, crate::time::now_local(rt.tz));
    if let Err(e) = rt.publisher.publish(topics::UserMessage, msg_event).await {
        tracing::warn!(error = %e, "failed to publish subconscious correction turn");
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use crate::subconscious::FindingKind;

    fn finding(severity: Severity, instruction: &str) -> Finding {
        Finding {
            kind: FindingKind::Omission,
            severity,
            instruction: instruction.to_string(),
        }
    }

    #[test]
    fn only_first_act_becomes_a_correction() {
        let plan = plan_delivery(vec![
            finding(Severity::Note, "note one"),
            finding(Severity::Act, "first act"),
            finding(Severity::Act, "second act"),
        ]);

        assert_eq!(
            plan,
            vec![
                Delivery::Note("note one".to_string()),
                Delivery::Correction("first act".to_string()),
                Delivery::Note("second act".to_string()),
            ],
            "exactly one correction per turn; later acts degrade to notes"
        );
    }

    #[test]
    fn all_notes_produce_no_correction() {
        let plan = plan_delivery(vec![
            finding(Severity::Note, "a"),
            finding(Severity::Note, "b"),
        ]);
        assert!(
            plan.iter().all(|d| matches!(d, Delivery::Note(_))),
            "note-only findings never trigger a correction turn"
        );
    }

    #[test]
    fn empty_findings_produce_empty_plan() {
        assert!(plan_delivery(vec![]).is_empty());
    }

    #[test]
    fn correction_event_has_background_origin() {
        let ts = chrono::NaiveDate::from_ymd_opt(2026, 2, 22)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();
        let event = build_correction_event("save the preference", "corr-123", ts);

        // The background origin is the loop-prevention guarantee: the end-of-turn
        // hook skips background turns, so this correction is never re-evaluated.
        assert_eq!(
            event.origin.endpoint, "background",
            "correction must use the background origin to avoid re-evaluation"
        );
        assert_eq!(event.origin.sender_name, "subconscious");
        assert_eq!(event.id, "subconscious-corr-123");
        assert!(
            event.content.contains("save the preference"),
            "instruction must reach the agent"
        );
    }
}
