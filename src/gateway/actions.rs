//! Scheduled action execution helper for the gateway.

use std::sync::Arc;

use crate::actions::store::ActionStore;
use crate::background::registry::{MAIN_DEPTH, generate_address};
use crate::bus::{EventTrigger, Publisher, SkillName, SpawnRequestEvent, topics};
use crate::config::BackgroundModelTier;

/// Fork due scheduled actions as `scheduled` sessions.
///
/// An action is removed from the store only once its spawn request has
/// actually been published — never before. An action whose publish fails
/// stays in the store exactly as it was, so it's retried on the next tick
/// rather than lost; the failure itself is already logged in
/// [`publish_action_spawn`].
pub(super) async fn spawn_due_actions(
    action_store: &Arc<tokio::sync::Mutex<ActionStore>>,
    publisher: &Publisher,
) {
    let now = chrono::Utc::now();
    let mut store = action_store.lock().await;
    let due = store.due(now);

    if due.is_empty() {
        return;
    }

    let mut started_ids = Vec::new();
    for action in &due {
        if publish_action_spawn(action, publisher).await {
            started_ids.push(action.id.clone());
        }
    }

    if started_ids.is_empty() {
        return;
    }

    for id in &started_ids {
        store.remove(id);
    }

    if let Err(e) = store.save().await {
        tracing::warn!(error = %e, "failed to save action store after spawning due actions");
    }
}

/// Publish a `SpawnRequest` for a scheduled action. Returns whether the
/// publish succeeded, so the caller knows whether the action's run actually
/// started and can be removed from the store.
async fn publish_action_spawn(
    action: &crate::actions::types::ScheduledAction,
    publisher: &Publisher,
) -> bool {
    let tier = action
        .model_tier
        .as_deref()
        .and_then(|s| s.parse().ok())
        .unwrap_or(BackgroundModelTier::Medium);

    let trigger = EventTrigger::Action;
    let address = generate_address(&trigger, &action.name);

    let spawn_event = SpawnRequestEvent {
        address,
        skill: action.agent.as_deref().map(SkillName::from),
        source_label: format!("action:{}", action.name),
        prompt: action.prompt.clone(),
        context: None,
        source: trigger,
        model_tier: tier,
        spawner: None,
        depth: MAIN_DEPTH + 1,
        hop_count: 0,
        sender: None,
        conversation: None,
        inbound: None,
        images: Vec::new(),
        overlap: None,
    };

    match publisher.publish(topics::Background, spawn_event).await {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!(
                action = %action.name,
                error = %e,
                "failed to publish action spawn request; action stays scheduled and will be \
                 retried on the next tick"
            );
            false
        }
    }
}
