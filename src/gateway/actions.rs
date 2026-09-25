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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::types::ScheduledAction;
    use crate::bus::Subscriber;
    use chrono::{Duration, Utc};

    fn due_action(id: &str, name: &str) -> ScheduledAction {
        let now = Utc::now();
        ScheduledAction {
            id: id.to_string(),
            name: name.to_string(),
            prompt: "do the thing".to_string(),
            run_at: now - Duration::seconds(5),
            agent: None,
            model_tier: None,
            created_at: now,
        }
    }

    #[tokio::test]
    async fn due_action_is_removed_only_after_a_successful_publish() {
        let dir = tempfile::tempdir().unwrap();
        let bus_handle = crate::bus::spawn_broker();
        let mut sub: Subscriber<SpawnRequestEvent> =
            bus_handle.subscribe(topics::Background).await.unwrap();

        let mut store = ActionStore::new_empty(dir.path().join("scheduled_actions.json"));
        store.add(due_action("action-aaaaaaaa", "morning digest"));
        let action_store = Arc::new(tokio::sync::Mutex::new(store));

        spawn_due_actions(&action_store, &bus_handle.publisher()).await;

        let event = tokio::time::timeout(std::time::Duration::from_millis(200), sub.recv())
            .await
            .expect("a spawn request should have been published")
            .unwrap()
            .unwrap();
        assert_eq!(event.source_label, "action:morning digest");

        let locked = action_store.lock().await;
        assert!(
            locked.list().is_empty(),
            "the action must be removed once its publish actually succeeded"
        );
    }

    #[tokio::test]
    async fn due_action_stays_in_the_store_when_publish_fails() {
        let dir = tempfile::tempdir().unwrap();
        // A publisher with no broker behind it: every publish fails with
        // `BusError::BrokerShutdown`, simulating the gateway shutting down
        // mid-tick.
        let publisher = Publisher::noop();

        let mut store = ActionStore::new_empty(dir.path().join("scheduled_actions.json"));
        store.add(due_action("action-bbbbbbbb", "nightly review"));
        let action_store = Arc::new(tokio::sync::Mutex::new(store));

        spawn_due_actions(&action_store, &publisher).await;

        let locked = action_store.lock().await;
        assert_eq!(
            locked.list().len(),
            1,
            "a failed publish must leave the action exactly as it was, to retry next tick"
        );
        assert_eq!(locked.list().first().unwrap().id, "action-bbbbbbbb");
    }

    #[tokio::test]
    async fn not_yet_due_action_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let publisher = Publisher::noop();

        let mut store = ActionStore::new_empty(dir.path().join("scheduled_actions.json"));
        let mut future = due_action("action-cccccccc", "future action");
        future.run_at = Utc::now() + Duration::hours(1);
        store.add(future);
        let action_store = Arc::new(tokio::sync::Mutex::new(store));

        spawn_due_actions(&action_store, &publisher).await;

        let locked = action_store.lock().await;
        assert_eq!(
            locked.list().len(),
            1,
            "a not-yet-due action must be untouched"
        );
    }
}
