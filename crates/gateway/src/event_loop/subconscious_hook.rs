use std::sync::{Arc, Mutex};

use crate::post_turn::SubconsciousTrigger;
use crate::types::AgentRuntime;
use residuum_agent::subconscious::TurnScratch;
use residuum_inference::Message;

pub(super) fn run_end_of_turn_subconscious(
    rt: &AgentRuntime,
    new_messages: &[Message],
    correlation_id: &str,
    scratch: Option<&Arc<Mutex<TurnScratch>>>,
) {
    if !rt.subconscious.enabled() {
        return;
    }
    if new_messages.len() < 2 {
        return;
    }

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
