//! Scheduled action execution helper for the gateway.

use std::sync::Arc;

use crate::actions::store::ActionStore;
use crate::background::registry::{MAIN_DEPTH, generate_address};
use crate::bus::{EventTrigger, Publisher, SkillName, SpawnRequestEvent, topics};
use crate::config::BackgroundModelTier;

/// Fork due scheduled actions as `scheduled` sessions.
///
/// Due actions are drained from the store and saved.
pub(super) async fn spawn_due_actions(
    action_store: &Arc<tokio::sync::Mutex<ActionStore>>,
    publisher: &Publisher,
) {
    let now = chrono::Utc::now();
    let mut store = action_store.lock().await;
    let due = store.take_due(now);

    if due.is_empty() {
        return;
    }

    for action in &due {
        publish_action_spawn(action, publisher).await;
    }

    if let Err(e) = store.save().await {
        tracing::warn!(error = %e, "failed to save action store after spawning due actions");
    }
}

/// Publish a `SpawnRequest` for a scheduled action.
async fn publish_action_spawn(
    action: &crate::actions::types::ScheduledAction,
    publisher: &Publisher,
) {
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
    };

    if let Err(e) = publisher.publish(topics::Background, spawn_event).await {
        tracing::warn!(
            action = %action.name,
            error = %e,
            "failed to publish action spawn request"
        );
    }
}
