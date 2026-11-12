//! Scheduled action execution helper for the gateway.

use std::sync::Arc;

use residuum_bus::notice::publish_notice;
use residuum_bus::session::{MAIN_DEPTH, generate_address};
use residuum_bus::{EventTrigger, Publisher, SkillName, SpawnRequestEvent, topics};
use residuum_config::{ModelCatalog, ModelSelector};
use residuum_proactivity::actions::store::ActionStore;

/// Fork due scheduled actions as `scheduled` sessions.
///
/// Every pending action's model is checked first (see [`action_model`]). An
/// action whose model can't run is held in the store and never started,
/// whether or not it's due yet, and the owner gets one notice naming it and
/// the bad value; it starts on the first tick after its model resolves (a
/// fixed `providers.toml`), or the owner cancels it.
///
/// An action whose spawn request is published is marked in flight — not due,
/// not listed, held out of the persisted file — and the fork's outcome comes
/// back through `SpawnContext::action_outcome`: a fork that started drops the
/// action for good; a fork that failed restores it verbatim, so it's retried
/// on the next tick rather than lost. The fork's own failure notice is
/// published by the listener, deduplicated. A crash between publish and
/// report can still drop an in-flight action; that window is the fork
/// itself, seconds, where before this the loss was certain on every fork
/// failure. A publish that fails leaves the action in the pending set
/// exactly as it was (also retried next tick); the failure itself is
/// already logged in [`publish_action_spawn`].
pub(super) async fn spawn_due_actions(
    action_store: &Arc<tokio::sync::Mutex<ActionStore>>,
    publisher: &Publisher,
    models: &ModelCatalog,
) {
    let now = chrono::Utc::now();
    let mut store = action_store.lock().await;

    let mut runnable = std::collections::HashMap::new();
    let mut problems = std::collections::HashMap::new();
    let mut remedies = std::collections::HashMap::new();
    for action in store.list() {
        match action_model(action, models) {
            Ok(model) => {
                runnable.insert(action.id.clone(), model);
            }
            Err(held) => {
                remedies.insert(action.id.clone(), held.remedy());
                problems.insert(action.id.clone(), held.reason);
            }
        }
    }
    for (name, id, reason) in store.record_model_problems(problems) {
        tracing::warn!(action = %name, id = %id, reason = %reason, "scheduled action's model can't run; holding it until it does");
        let remedy = remedies.get(&id).copied().unwrap_or_default();
        publish_notice(
            publisher,
            format!("The scheduled action \"{name}\" (id {id}) can't run: {reason}. {remedy}"),
        )
        .await;
    }

    let due: Vec<_> = store
        .due(now)
        .into_iter()
        .filter_map(|action| {
            let model = runnable.remove(&action.id)?;
            Some((action, model))
        })
        .collect();

    if due.is_empty() {
        return;
    }

    let mut started = false;
    for (action, model) in due {
        if publish_action_spawn(&action, model, publisher).await {
            store.mark_in_flight(&action.id);
            started = true;
        }
    }

    if !started {
        return;
    }

    if let Err(e) = store.save().await {
        tracing::warn!(error = %e, "failed to save action store after spawning due actions");
    }
}

/// The fork-outcome sink wired into the gateway's `SpawnContext`: keeps a
/// scheduled action alive across its fork — restored verbatim to retry when
/// the fork fails, dropped once it starts — and persists the store either
/// way. `source_label` is the spawn request's `action:<name>#<id>` label;
/// the id suffix resolves one exact action, so same-named actions can't
/// clobber each other's outcomes.
pub(super) fn action_outcome_sink(
    action_store: &Arc<tokio::sync::Mutex<ActionStore>>,
) -> residuum_background::ActionOutcomeSink {
    let store = Arc::clone(action_store);
    Arc::new(move |source_label, outcome| {
        let store = Arc::clone(&store);
        let label = source_label
            .strip_prefix("action:")
            .unwrap_or(source_label)
            .to_string();
        Box::pin(async move {
            let mut locked = store.lock().await;
            let (name, id) = label
                .rsplit_once('#')
                .map_or((label.as_str(), None), |(name, id)| {
                    (name, Some(id.to_string()))
                });
            match (outcome, id) {
                (residuum_background::ActionSpawnOutcome::Started, Some(id)) => {
                    locked.drop_in_flight_by_id(&id);
                }
                (residuum_background::ActionSpawnOutcome::Failed, Some(id)) => {
                    locked.restore_in_flight_by_id(&id);
                }
                (outcome, None) => {
                    tracing::warn!(
                        label = %label,
                        "action spawn label carries no id; falling back to a name match"
                    );
                    match outcome {
                        residuum_background::ActionSpawnOutcome::Started => {
                            locked.drop_in_flight_by_name(name);
                        }
                        residuum_background::ActionSpawnOutcome::Failed => {
                            locked.restore_in_flight_by_name(name);
                        }
                    }
                }
            }
            if let Err(e) = locked.save().await {
                tracing::warn!(
                    error = %e,
                    label = %label,
                    "failed to save the action store after a fork outcome"
                );
            }
        })
    })
}

/// Why a scheduled action's model can't run, which decides what the owner
/// is told to do about it.
struct HeldModel {
    /// Plain-language reason naming the value.
    reason: String,
    /// `true` when the value is a `provider/model` id whose provider isn't
    /// configured, which adding the provider fixes; `false` when the value
    /// isn't a tier or a model id at all, which only rescheduling fixes.
    fixable_by_provider: bool,
}

impl HeldModel {
    /// What the owner can do about it.
    fn remedy(&self) -> &'static str {
        if self.fixable_by_provider {
            "It stays scheduled and starts as soon as its model works — add the provider to \
             providers.toml, or cancel the action and schedule it again with a different model."
        } else {
            "It won't run as scheduled — cancel it in the Schedule place and schedule it again \
             with a tier (small, medium, or large) or a provider/model id."
        }
    }
}

/// An action's model, checked against `models`.
///
/// # Errors
/// Returns a [`HeldModel`] naming the value when `model_tier` is neither a
/// tier nor a `provider/model` id, or names a model that doesn't resolve.
fn action_model(
    action: &residuum_proactivity::actions::types::ScheduledAction,
    models: &ModelCatalog,
) -> Result<ModelSelector, HeldModel> {
    let model = action.model().map_err(|reason| HeldModel {
        reason,
        fixable_by_provider: false,
    })?;
    models.check(&model).map_err(|reason| HeldModel {
        reason,
        fixable_by_provider: true,
    })?;
    Ok(model)
}

/// Publish a `SpawnRequest` for a scheduled action. Returns whether the
/// publish succeeded, so the caller knows whether the action's run actually
/// started and can be removed from the store.
async fn publish_action_spawn(
    action: &residuum_proactivity::actions::types::ScheduledAction,
    model: ModelSelector,
    publisher: &Publisher,
) -> bool {
    let trigger = EventTrigger::Action;
    let address = generate_address(&trigger, &action.name);

    let spawn_event = SpawnRequestEvent {
        address,
        skill: action.agent.as_deref().map(SkillName::from),
        source_label: format!("action:{}#{}", action.name, action.id),
        prompt: action.prompt.clone(),
        context: None,
        source: trigger,
        model,
        spawner: None,
        depth: MAIN_DEPTH + 1,
        hop_count: 0,
        sender: None,
        conversation: None,
        inbound: None,
        images: Vec::new(),
        overlap: None,
        carried_history: Vec::new(),
        fire_id: None,
        delivered_messages: Vec::new(),
        delivered_chat_messages: Vec::new(),
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
    use chrono::{Duration, Utc};
    use residuum_bus::Subscriber;
    use residuum_bus::testing::next_event;
    use residuum_proactivity::actions::types::ScheduledAction;

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
    async fn due_action_goes_in_flight_only_after_a_successful_publish() {
        let dir = tempfile::tempdir().unwrap();
        let bus_handle = residuum_bus::spawn_broker();
        let mut sub: Subscriber<SpawnRequestEvent> =
            bus_handle.subscribe(topics::Background).await.unwrap();

        let mut store = ActionStore::new_empty(dir.path().join("scheduled_actions.json"));
        store.add(due_action("action-aaaaaaaa", "morning digest"));
        let action_store = Arc::new(tokio::sync::Mutex::new(store));

        spawn_due_actions(
            &action_store,
            &bus_handle.publisher(),
            &ModelCatalog::default(),
        )
        .await;

        let event = next_event("a spawn request", &mut sub).await;
        assert_eq!(event.source_label, "action:morning digest#action-aaaaaaaa");

        let locked = action_store.lock().await;
        assert!(
            locked.list().is_empty() && locked.in_flight() == 1,
            "the action must be in flight once its publish actually succeeded, \
             held there until its fork reports back"
        );
    }

    #[tokio::test]
    async fn an_action_whose_fork_failed_is_restored_to_retry() {
        let dir = tempfile::tempdir().unwrap();
        let bus_handle = residuum_bus::spawn_broker();
        let mut sub: Subscriber<SpawnRequestEvent> =
            bus_handle.subscribe(topics::Background).await.unwrap();

        let mut store = ActionStore::new_empty(dir.path().join("scheduled_actions.json"));
        let original = due_action("action-aaaaaaaa", "morning digest");
        store.add(original);
        let action_store = Arc::new(tokio::sync::Mutex::new(store));

        spawn_due_actions(
            &action_store,
            &bus_handle.publisher(),
            &ModelCatalog::default(),
        )
        .await;
        let _ = next_event("a spawn request", &mut sub).await;

        // The fork failed: the listener reports back by the spawn's
        // `action:<name>#<id>` label, and the action returns verbatim to the
        // pending set — the next tick retries it instead of losing it.
        {
            let mut locked = action_store.lock().await;
            assert!(
                locked.restore_in_flight_by_id("action-aaaaaaaa"),
                "the failed fork's action must come back"
            );
            locked.save().await.unwrap();
        }
        drop(sub);
        spawn_due_actions(&action_store, &Publisher::noop(), &ModelCatalog::default()).await;

        let locked = action_store.lock().await;
        assert_eq!(locked.in_flight(), 0);
        assert_eq!(locked.list().len(), 1, "restored to retry on the next tick");
        let retried = locked.list().first().unwrap();
        assert_eq!(retried.id, "action-aaaaaaaa", "restored verbatim");
        assert_eq!(retried.name, "morning digest");
    }

    #[tokio::test]
    async fn an_action_whose_fork_started_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let bus_handle = residuum_bus::spawn_broker();
        let mut sub: Subscriber<SpawnRequestEvent> =
            bus_handle.subscribe(topics::Background).await.unwrap();

        let mut store = ActionStore::new_empty(dir.path().join("scheduled_actions.json"));
        store.add(due_action("action-aaaaaaaa", "morning digest"));
        let action_store = Arc::new(tokio::sync::Mutex::new(store));

        spawn_due_actions(
            &action_store,
            &bus_handle.publisher(),
            &ModelCatalog::default(),
        )
        .await;
        let _ = next_event("a spawn request", &mut sub).await;

        let mut locked = action_store.lock().await;
        assert!(locked.drop_in_flight_by_id("action-aaaaaaaa"));
        assert_eq!(locked.in_flight(), 0);
        assert!(
            locked.list().is_empty(),
            "a started action is gone for good"
        );
        locked.save().await.unwrap();
    }

    #[tokio::test]
    async fn same_named_actions_resolve_by_id_not_name() {
        let dir = tempfile::tempdir().unwrap();
        let bus_handle = residuum_bus::spawn_broker();
        let mut sub: Subscriber<SpawnRequestEvent> =
            bus_handle.subscribe(topics::Background).await.unwrap();

        let mut store = ActionStore::new_empty(dir.path().join("scheduled_actions.json"));
        store.add(due_action("action-aaaaaaaa", "check"));
        store.add(due_action("action-bbbbbbbb", "check"));
        let action_store = Arc::new(tokio::sync::Mutex::new(store));
        let sink = action_outcome_sink(&action_store);

        spawn_due_actions(
            &action_store,
            &bus_handle.publisher(),
            &ModelCatalog::default(),
        )
        .await;
        let first = next_event("the first check spawn", &mut sub).await;
        let second = next_event("the second check spawn", &mut sub).await;
        assert_ne!(
            first.source_label, second.source_label,
            "the id suffix must disambiguate same-named actions"
        );

        // One fork started, one failed: each outcome must touch exactly its
        // own action, never its same-named sibling.
        (sink)(
            first.source_label.as_str(),
            residuum_background::ActionSpawnOutcome::Started,
        )
        .await;
        (sink)(
            second.source_label.as_str(),
            residuum_background::ActionSpawnOutcome::Failed,
        )
        .await;

        let locked = action_store.lock().await;
        assert_eq!(locked.in_flight(), 0);
        assert_eq!(
            locked.list().len(),
            1,
            "exactly the failed fork's action is restored to retry"
        );
        assert_eq!(
            locked.list().first().unwrap().id,
            "action-bbbbbbbb",
            "resolved by id, not by name"
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

        spawn_due_actions(&action_store, &publisher, &ModelCatalog::default()).await;

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

        spawn_due_actions(&action_store, &publisher, &ModelCatalog::default()).await;

        let locked = action_store.lock().await;
        assert_eq!(
            locked.list().len(),
            1,
            "a not-yet-due action must be untouched"
        );
    }

    #[tokio::test]
    async fn action_with_an_unusable_model_is_held_and_noticed_once() {
        let dir = tempfile::tempdir().unwrap();
        let bus_handle = residuum_bus::spawn_broker();
        let mut notices: Subscriber<residuum_bus::NoticeEvent> = bus_handle
            .subscribe(topics::Notification(residuum_bus::NotifyName::from(
                residuum_bus::SYSTEM_CHANNEL,
            )))
            .await
            .unwrap();
        let mut spawns: Subscriber<SpawnRequestEvent> =
            bus_handle.subscribe(topics::Background).await.unwrap();

        let mut store = ActionStore::new_empty(dir.path().join("scheduled_actions.json"));
        let mut typo = due_action("action-dddddddd", "typo tier");
        typo.model_tier = Some("lagre".to_string());
        store.add(typo);
        let mut unknown = due_action("action-eeeeeeee", "unknown provider");
        unknown.model_tier = Some("nowhere/some-model".to_string());
        store.add(unknown);
        let action_store = Arc::new(tokio::sync::Mutex::new(store));

        spawn_due_actions(
            &action_store,
            &bus_handle.publisher(),
            &ModelCatalog::default(),
        )
        .await;

        let mut texts = vec![
            next_event("the first held-action notice", &mut notices)
                .await
                .message,
            next_event("the second held-action notice", &mut notices)
                .await
                .message,
        ];
        texts.sort();
        assert!(
            texts.iter().any(|t| t.contains("typo tier")
                && t.contains("'lagre'")
                && t.contains("schedule it again")
                && !t.contains("providers.toml")),
            "an unreadable value is fixed by rescheduling, not by providers.toml: {texts:?}"
        );
        assert!(
            texts.iter().any(|t| t.contains("unknown provider")
                && t.contains("nowhere")
                && t.contains("providers.toml")),
            "{texts:?}"
        );
        assert_eq!(
            action_store.lock().await.list().len(),
            2,
            "an action whose model can't run stays scheduled"
        );

        // A second tick with nothing changed starts nothing and says nothing
        // new. The valid action added before it does start: the broker
        // forwards in publish order, so once its spawn arrives any notice
        // this tick published would already be queued.
        let mut good = due_action("action-ffffffff", "explicit model");
        good.model_tier = Some("openai/gpt-4o".to_string());
        action_store.lock().await.add(good);
        spawn_due_actions(
            &action_store,
            &bus_handle.publisher(),
            &ModelCatalog::default(),
        )
        .await;
        let event = next_event("the valid action's spawn", &mut spawns).await;
        assert_eq!(event.source_label, "action:explicit model#action-ffffffff");
        assert_eq!(
            event.model,
            ModelSelector::Explicit("openai/gpt-4o".to_string())
        );
        assert!(
            notices.drain().is_empty(),
            "an unchanged problem must not be noticed again"
        );
    }
}
