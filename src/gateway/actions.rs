//! Scheduled action execution helper for the gateway.

use std::sync::Arc;

use crate::actions::store::ActionStore;
use crate::bus::{EventTrigger, PresetName, Publisher, SpawnRequestEvent, TopicId};
use crate::config::BackgroundModelTier;

/// A scheduled action that should run as a main agent wake turn rather than a sub-agent.
pub(super) struct ActionMainTurn {
    /// Action name (for logging/formatting).
    pub action_name: String,
    /// The prompt to inject.
    pub prompt: String,
}

/// Spawn due scheduled actions as background tasks, returning any that need main agent turns.
///
/// - `agent = Some("main")` actions are returned as `ActionMainTurn`s.
/// - `agent = Some(preset)` actions publish a `SpawnRequest` to the preset topic.
/// - `agent = None` actions publish a `SpawnRequest` to `general-purpose`.
///
/// Due actions are drained from the store and saved.
pub(super) async fn spawn_due_actions(
    action_store: &Arc<tokio::sync::Mutex<ActionStore>>,
    publisher: &Publisher,
) -> Vec<ActionMainTurn> {
    let now = chrono::Utc::now();
    let mut store = action_store.lock().await;
    let due = store.take_due(now);

    if due.is_empty() {
        return Vec::new();
    }

    let mut main_turns = Vec::new();

    for action in &due {
        match action.agent.as_deref() {
            Some("main") => {
                main_turns.push(ActionMainTurn {
                    action_name: action.name.clone(),
                    prompt: action.prompt.clone(),
                });
            }
            Some(preset_name) => {
                publish_action_spawn(action, preset_name, publisher).await;
            }
            None => {
                publish_action_spawn(action, "general-purpose", publisher).await;
            }
        }
    }

    if let Err(e) = store.save().await {
        tracing::warn!(error = %e, "failed to save action store after spawning due actions");
    }

    main_turns
}

/// Publish a `SpawnRequest` for a scheduled action to the appropriate preset topic.
async fn publish_action_spawn(
    action: &crate::actions::types::ScheduledAction,
    preset_name: &str,
    publisher: &Publisher,
) {
    let tier = action
        .model_tier
        .as_deref()
        .and_then(|s| s.parse().ok())
        .unwrap_or(BackgroundModelTier::Medium);

    let spawn_event = SpawnRequestEvent {
        task_name: action.name.clone(),
        prompt: action.prompt.clone(),
        context: None,
        source: EventTrigger::Action,
        model_tier_override: Some(tier),
        routing_override: Some(action.channels.clone()),
    };

    let topic = TopicId::AgentPreset(PresetName::from(preset_name));
    if let Err(e) = publisher
        .publish(topic, crate::bus::BusEvent::SpawnRequest(spawn_event))
        .await
    {
        tracing::warn!(
            action = %action.name,
            error = %e,
            "failed to publish action spawn request"
        );
    }
}
