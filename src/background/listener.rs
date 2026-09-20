//! Bus listener that turns spawn requests into running sub-agents.
//!
//! Subscribes to the `Background` topic and, for each `SpawnRequestEvent`,
//! builds isolated resources and hands the task to `BackgroundTaskSpawner`.
//! Everything the sub-agent runs with — model tier, skill, identity — comes
//! from the request itself; there is no resolution step in between.

use std::sync::Arc;

use rand::Rng;
use tokio::task::JoinHandle;

use crate::background::spawn_context::{SpawnContext, build_spawn_resources};
use crate::background::types::{BackgroundTask, SubAgentConfig};
use crate::bus::{BusHandle, SpawnRequestEvent, Subscriber, topics};

/// Subscribe to the `Background` topic and spawn sub-agents on demand.
///
/// Returns a `JoinHandle` for shutdown coordination. The task runs until the
/// bus shuts down or the subscriber closes.
#[tracing::instrument(skip_all)]
pub(crate) async fn spawn_listener(
    ctx: Arc<SpawnContext>,
    bus_handle: &BusHandle,
) -> Option<JoinHandle<()>> {
    let subscriber: Subscriber<SpawnRequestEvent> = match bus_handle
        .subscribe(topics::Background)
        .await
    {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, "spawn listener failed to subscribe to Background topic");
            return None;
        }
    };

    tracing::info!("spawn listener subscribed to Background topic");

    Some(tokio::spawn(listener_loop(ctx, subscriber)))
}

/// Main loop: reads spawn requests and executes them.
async fn listener_loop(ctx: Arc<SpawnContext>, mut subscriber: Subscriber<SpawnRequestEvent>) {
    loop {
        match subscriber.recv().await {
            Ok(Some(event)) => {
                let source_label = event.source_label.clone();
                if let Err(e) = handle_spawn_request(&ctx, event).await {
                    tracing::warn!(
                        source = %source_label,
                        error = %e,
                        "failed to spawn sub-agent"
                    );
                }
            }
            Ok(None) => break,
            Err(e) => {
                tracing::error!(error = %e, "spawn listener subscriber error, shutting down");
                break;
            }
        }
    }
    tracing::info!("spawn listener shutting down");
}

/// Handle a single spawn request: build resources, spawn the task.
async fn handle_spawn_request(
    ctx: &Arc<SpawnContext>,
    event: SpawnRequestEvent,
) -> Result<(), anyhow::Error> {
    let skill = event.skill.as_ref().map(|s| s.as_ref().to_string());

    let resources = build_spawn_resources(
        ctx,
        &event.model_tier,
        skill.as_deref(),
        event.include_identity,
    )
    .await?;

    let task = BackgroundTask {
        id: generate_task_id(skill.as_deref().unwrap_or("subagent")),
        source_label: event.source_label,
        source: event.source,
        subagent_config: SubAgentConfig {
            prompt: event.prompt,
            context: event.context,
            model_tier: event.model_tier,
        },
        agent_skill: event.skill,
    };

    let log_task_id = task.id.clone();
    let log_source_label = task.source_label.clone();
    ctx.background_spawner.spawn(task, Some(resources)).await?;
    tracing::info!(
        skill = skill.as_deref().unwrap_or("none"),
        task_id = %log_task_id,
        source = %log_source_label,
        "spawned sub-agent"
    );
    Ok(())
}

/// Generate a task ID from a prefix plus timestamp and random suffix.
fn generate_task_id(prefix: &str) -> String {
    let timestamp_ms = chrono::Utc::now().timestamp_millis();
    let rand_part: u32 = rand::thread_rng().r#gen();
    format!("{prefix}-{timestamp_ms}-{rand_part:08x}")
}

#[cfg(test)]
mod tests {
    use super::generate_task_id;

    #[test]
    fn task_id_contains_prefix() {
        let id = generate_task_id("memory-analyst");
        assert!(
            id.starts_with("memory-analyst-"),
            "id should start with the prefix"
        );
    }

    #[test]
    fn task_ids_are_unique() {
        let id1 = generate_task_id("x");
        let id2 = generate_task_id("x");
        assert_ne!(id1, id2, "two ids for the same prefix must be distinct");
    }
}
