//! End-of-turn subconscious evaluation and finding delivery.
//!
//! Runs after a user-visible turn completes. `note` findings are injected as
//! passive context for the next turn; the first `act` finding triggers an
//! immediate correction turn by publishing a background-origin `MessageEvent`
//! — the same mechanism the notification router uses to wake the main agent.
//! Background turns are never evaluated, which is what prevents a correction
//! turn from triggering another evaluation.

use std::sync::Arc;

use crate::bus::topics;
use crate::gateway::types::GatewayRuntime;
use crate::models::Message;
use crate::subconscious::{EvalPhase, Finding, Severity};

/// Evaluate a completed turn and deliver any findings to the agent.
///
/// Never fails the turn: evaluation errors are logged and swallowed.
pub(super) async fn run_end_of_turn_subconscious(
    rt: &mut GatewayRuntime,
    new_messages: &[Message],
    correlation_id: &str,
) {
    if !rt.subconscious.enabled() {
        return;
    }
    // A turn with no assistant output (e.g. hard error) has nothing to judge.
    if new_messages.len() < 2 {
        return;
    }

    let subconscious = Arc::clone(&rt.subconscious);
    match subconscious
        .evaluate(new_messages, EvalPhase::EndOfTurn)
        .await
    {
        Ok(findings) => deliver_findings(rt, findings, correlation_id).await,
        Err(e) => {
            tracing::warn!(error = %e, "subconscious end-of-turn evaluation failed");
        }
    }
}

/// Deliver findings: notes become passive context, the first `act` finding
/// triggers a correction turn. Remaining `act` findings degrade to notes so a
/// single turn never spawns more than one correction.
async fn deliver_findings(rt: &mut GatewayRuntime, findings: Vec<Finding>, correlation_id: &str) {
    let mut act_delivered = false;
    for finding in findings {
        if finding.severity == Severity::Act && !act_delivered {
            act_delivered = true;
            publish_correction_turn(rt, &finding, correlation_id).await;
        } else {
            tracing::info!(
                kind = ?finding.kind,
                "subconscious note queued for next turn"
            );
            rt.agent
                .inject_system_message(format!("[Subconscious note] {}", finding.instruction));
        }
    }
}

/// Publish a background-origin `MessageEvent` that starts a correction turn.
async fn publish_correction_turn(rt: &GatewayRuntime, finding: &Finding, correlation_id: &str) {
    let content = format!(
        "[Subconscious] A background check of your last turn found a problem to correct now:\n\
         {}\n\
         Take the corrective action (e.g. update MEMORY.md). Only message the user if they \
         need to know something new.",
        finding.instruction
    );

    let msg_event = crate::bus::MessageEvent {
        id: format!("subconscious-{correlation_id}"),
        content,
        origin: crate::interfaces::types::MessageOrigin {
            endpoint: "background".to_string(),
            sender_name: "subconscious".to_string(),
            sender_id: correlation_id.to_string(),
        },
        timestamp: crate::time::now_local(rt.tz),
        images: vec![],
    };

    tracing::info!(
        kind = ?finding.kind,
        "subconscious act finding triggering correction turn"
    );
    if let Err(e) = rt.publisher.publish(topics::UserMessage, msg_event).await {
        tracing::warn!(error = %e, "failed to publish subconscious correction turn");
    }
}
