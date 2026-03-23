//! Subagent registry: a bus participant that spawns sub-agents on demand.
//!
//! Subscribes to the `Background` topic for `SpawnRequestEvent` events and
//! handles them by loading the preset, building resources, and calling
//! `BackgroundTaskSpawner::spawn()`.

use std::path::PathBuf;
use std::sync::Arc;

use rand::Rng;
use tokio::task::JoinHandle;

use crate::background::BackgroundTaskSpawner;
use crate::background::spawn_context::{
    SpawnContext, build_spawn_resources, load_preset_for_spawn,
};
use crate::background::types::{BackgroundTask, SubAgentConfig};
use crate::bus::{BusHandle, PresetName, SpawnRequestEvent, Subscriber, topics};
use crate::config::BackgroundModelTier;
use crate::mcp::SharedMcpRegistry;
use crate::projects::activation::SharedProjectState;
use crate::skills::SharedSkillState;

/// Holds references needed to spawn sub-agents on behalf of bus callers.
pub struct SubagentRegistry {
    spawner: Arc<BackgroundTaskSpawner>,
    spawn_context: Arc<SpawnContext>,
    project_state: SharedProjectState,
    skill_state: SharedSkillState,
    mcp_registry: SharedMcpRegistry,
    subagents_dir: PathBuf,
}

impl SubagentRegistry {
    /// Create a new registry with all subsystem references.
    #[must_use]
    pub(crate) fn new(
        spawner: Arc<BackgroundTaskSpawner>,
        spawn_context: Arc<SpawnContext>,
        project_state: SharedProjectState,
        skill_state: SharedSkillState,
        mcp_registry: SharedMcpRegistry,
        subagents_dir: PathBuf,
    ) -> Self {
        Self {
            spawner,
            spawn_context,
            project_state,
            skill_state,
            mcp_registry,
            subagents_dir,
        }
    }
}

/// Spawn the registry task that subscribes to the `Background` topic.
///
/// Returns a `JoinHandle` for shutdown coordination. The task runs until the
/// bus shuts down or the subscriber closes.
#[tracing::instrument(skip_all)]
pub async fn spawn_registry(
    registry: SubagentRegistry,
    bus_handle: &BusHandle,
) -> Option<JoinHandle<()>> {
    let subscriber: Subscriber<SpawnRequestEvent> = match bus_handle
        .subscribe(topics::Background)
        .await
    {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, "subagent registry failed to subscribe to Background topic");
            return None;
        }
    };

    tracing::info!("subagent registry subscribed to Background topic");

    Some(tokio::spawn(registry_loop(registry, subscriber)))
}

/// Main loop: reads spawn requests and executes them.
async fn registry_loop(registry: SubagentRegistry, mut subscriber: Subscriber<SpawnRequestEvent>) {
    tracing::info!("subagent registry started");
    loop {
        match subscriber.recv().await {
            Ok(Some(event)) => {
                let source_label = event.source_label.clone();
                let preset_name = event.preset.as_ref().to_string();
                if let Err(e) = handle_spawn_request(&registry, event).await {
                    tracing::warn!(
                        preset = %preset_name,
                        source = %source_label,
                        error = %e,
                        "subagent registry failed to handle spawn request"
                    );
                }
            }
            Ok(None) => break,
            Err(e) => {
                tracing::error!(error = %e, "subagent registry subscriber error, shutting down");
                break;
            }
        }
    }
    tracing::info!("subagent registry shutting down");
}

/// Handle a single spawn request: load preset, build resources, spawn task.
async fn handle_spawn_request(
    registry: &SubagentRegistry,
    event: SpawnRequestEvent,
) -> Result<(), anyhow::Error> {
    let preset_name = event.preset.as_ref().to_string();

    let preset_result = load_preset_for_spawn(
        &registry.subagents_dir,
        &preset_name,
        BackgroundModelTier::Medium,
    )
    .await;

    let (preset_tier, preset_fm, preset_body, effective_preset_name) = match preset_result {
        Ok((tier, fm, body)) => (tier, fm, body, preset_name.clone()),
        Err(e) => {
            tracing::warn!(
                preset = %preset_name,
                error = %e,
                "failed to load preset, falling back to general-purpose"
            );
            let (tier, fm, body) = load_preset_for_spawn(
                &registry.subagents_dir,
                "general-purpose",
                BackgroundModelTier::Medium,
            )
            .await?;
            (tier, fm, body, "general-purpose".to_string())
        }
    };

    // Use override tier if provided, otherwise use preset-resolved tier
    let final_tier = event.model_tier_override.unwrap_or(preset_tier);

    let resources = build_spawn_resources(
        &registry.spawn_context,
        &final_tier,
        &registry.project_state,
        &registry.skill_state,
        Arc::clone(&registry.mcp_registry),
        Some((&preset_fm, preset_body)),
    )
    .await?;

    let task_id = generate_registry_task_id(&effective_preset_name);
    let task = BackgroundTask {
        id: task_id,
        source_label: event.source_label,
        source: event.source,
        subagent_config: SubAgentConfig {
            prompt: event.prompt,
            context: event.context,
            model_tier: final_tier,
        },
        agent_preset: PresetName::from(effective_preset_name.as_str()),
    };

    let log_task_id = task.id.clone();
    let log_source_label = task.source_label.clone();
    registry.spawner.spawn(task, Some(resources)).await?;
    tracing::info!(
        preset = %effective_preset_name,
        task_id = %log_task_id,
        source = %log_source_label,
        "spawned subagent"
    );
    Ok(())
}

/// Generate a task ID for a registry-spawned task.
fn generate_registry_task_id(preset_name: &str) -> String {
    let timestamp_ms = chrono::Utc::now().timestamp_millis();
    let rand_part: u32 = rand::thread_rng().r#gen();
    format!("{preset_name}-{timestamp_ms}-{rand_part:08x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_task_id_contains_preset() {
        let id = generate_registry_task_id("researcher");
        assert!(
            id.starts_with("researcher-"),
            "id should start with preset name"
        );
    }

    #[test]
    fn registry_task_ids_are_unique() {
        let id1 = generate_registry_task_id("x");
        let id2 = generate_registry_task_id("x");
        assert_ne!(
            id1, id2,
            "two ids for the same preset name must be distinct"
        );
    }
}
