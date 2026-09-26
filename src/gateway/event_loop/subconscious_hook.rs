//! End-of-turn subconscious evaluation trigger.
//!
//! Runs after a user-visible turn completes. The actual evaluation (an LLM
//! call) happens in the background — see `crate::gateway::post_turn` — so
//! this module's only job is building the trigger payload from whatever the
//! mid-turn watch already applied (via `TurnScratch`, so the background
//! evaluation can act as a triage step — not repeating corrections it
//! already delivered and folding in queued notes — rather than a blind
//! re-classification of the same turn) and firing it. `note` findings are
//! delivered as passive context for the agent's next turn once the
//! background cycle finishes; the first `act` finding triggers an immediate
//! correction turn directly from the background task. Background turns are
//! never evaluated, which is what prevents a correction turn from
//! triggering another evaluation.

use std::sync::{Arc, Mutex};

use crate::gateway::post_turn::SubconsciousTrigger;
use crate::gateway::types::GatewayRuntime;
use crate::inference::Message;
use crate::subconscious::TurnScratch;

/// Snapshot whatever the background subconscious worker needs and trigger
/// it. Never blocks: the actual evaluation runs off the event loop, and its
/// findings are applied (or, for a correction, published) from there too —
/// see `crate::gateway::post_turn::SubconsciousWorker`.
pub(super) fn run_end_of_turn_subconscious(
    rt: &GatewayRuntime,
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

    rt.post_turn_subconscious.trigger(SubconsciousTrigger {
        subconscious: Arc::clone(&rt.subconscious),
        learning_state: Arc::clone(&rt.learning_state),
        publisher: rt.publisher.clone(),
        tz: rt.tz,
        learning_cooldown: rt.cfg.subconscious_settings.learning_cooldown(),
        new_messages: new_messages.to_vec(),
        correlation_id: correlation_id.to_string(),
        scratch: prior,
    });
}
