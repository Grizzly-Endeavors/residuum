//! Subagent registry: a bus participant that spawns sub-agents on demand.
//!
//! Subscribes to every `TopicId::AgentPreset(name)` topic and handles
//! `BusEvent::SpawnRequest` events by loading the preset, building resources,
//! and calling `BackgroundTaskSpawner::spawn()`.

use std::path::PathBuf;
use std::sync::Arc;

use rand::Rng;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::background::BackgroundTaskSpawner;
use crate::background::spawn_context::{
    SpawnContext, build_spawn_resources, load_preset_for_spawn,
};
use crate::background::types::{BackgroundTask, Execution, SubAgentConfig};
use crate::bus::{BusEvent, BusHandle, PresetName, SpawnRequestEvent, TopicId};
use crate::config::BackgroundModelTier;
use crate::mcp::SharedMcpRegistry;
use crate::projects::activation::SharedProjectState;
use crate::skills::SharedSkillState;
use crate::subagents::SubagentPresetIndex;

/// Holds references needed to spawn sub-agents on behalf of bus callers.
pub struct SubagentRegistry {
    spawner: Arc<BackgroundTaskSpawner>,
    spawn_context: Arc<SpawnContext>,
    project_state: SharedProjectState,
    skill_state: SharedSkillState,
    mcp_registry: SharedMcpRegistry,
    subagents_dir: PathBuf,
}

/// A spawn request tagged with the preset name it was published to.
struct TaggedRequest {
    preset_name: String,
    event: SpawnRequestEvent,
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

/// Spawn the registry task that subscribes to all preset topics.
///
/// Returns a `JoinHandle` for shutdown coordination. The task runs until the
/// bus shuts down or the shared channel closes.
pub async fn spawn_registry(registry: SubagentRegistry, bus_handle: &BusHandle) -> JoinHandle<()> {
    let (fwd_tx, fwd_rx) = mpsc::channel::<TaggedRequest>(64);

    // Scan known presets and subscribe to each topic
    let index = match SubagentPresetIndex::scan(&registry.subagents_dir).await {
        Ok(idx) => idx,
        Err(e) => {
            tracing::warn!(error = %e, "subagent registry failed to scan presets, running with built-ins only");
            SubagentPresetIndex::default()
        }
    };

    for entry in index.entries() {
        let topic = TopicId::AgentPreset(PresetName::from(entry.name.as_str()));
        match bus_handle.subscribe(topic.clone()).await {
            Ok(subscriber) => {
                let tx = fwd_tx.clone();
                let preset_name = entry.name.clone();
                tokio::spawn(forward_subscription(subscriber, tx, preset_name));
            }
            Err(e) => {
                tracing::warn!(
                    preset = %entry.name,
                    error = %e,
                    "failed to subscribe to preset topic"
                );
            }
        }
    }

    // Drop the original sender — forwarding tasks hold their own clones
    drop(fwd_tx);

    tokio::spawn(registry_loop(registry, fwd_rx))
}

/// Forward `SpawnRequest` events from a single preset subscription to the shared channel.
async fn forward_subscription(
    mut subscriber: crate::bus::Subscriber,
    tx: mpsc::Sender<TaggedRequest>,
    preset_name: String,
) {
    while let Some(event) = subscriber.recv().await {
        if let BusEvent::SpawnRequest(spawn_event) = event {
            let tagged = TaggedRequest {
                preset_name: preset_name.clone(),
                event: spawn_event,
            };
            if tx.send(tagged).await.is_err() {
                break;
            }
        } else {
            tracing::trace!(
                preset = %preset_name,
                "ignoring non-SpawnRequest event on preset topic"
            );
        }
    }
}

/// Main loop: reads tagged spawn requests and executes them.
async fn registry_loop(registry: SubagentRegistry, mut rx: mpsc::Receiver<TaggedRequest>) {
    while let Some(tagged) = rx.recv().await {
        if let Err(e) = handle_spawn_request(&registry, &tagged.preset_name, tagged.event).await {
            tracing::warn!(
                preset = %tagged.preset_name,
                error = %e,
                "subagent registry failed to handle spawn request"
            );
        }
    }
    tracing::info!("subagent registry shutting down");
}

/// Handle a single spawn request: load preset, build resources, spawn task.
async fn handle_spawn_request(
    registry: &SubagentRegistry,
    preset_name: &str,
    event: SpawnRequestEvent,
) -> Result<(), anyhow::Error> {
    let (preset_tier, preset) = load_preset_for_spawn(
        &registry.subagents_dir,
        preset_name,
        BackgroundModelTier::Medium,
    )
    .await?;

    // Use override tier if provided, otherwise use preset-resolved tier
    let final_tier = event.model_tier_override.unwrap_or(preset_tier);

    let preset_arg = preset.as_ref().map(|(fm, body)| (fm, body.clone()));

    let resources = build_spawn_resources(
        &registry.spawn_context,
        &final_tier,
        &registry.project_state,
        &registry.skill_state,
        Arc::clone(&registry.mcp_registry),
        preset_arg,
    )
    .await?;

    let task_id = generate_registry_task_id(preset_name);
    let task = BackgroundTask {
        id: task_id,
        source_label: event.source_label,
        source: event.source,
        execution: Execution::SubAgent(SubAgentConfig {
            prompt: event.prompt,
            context: event.context,
            model_tier: final_tier,
        }),
        agent_preset: PresetName::from(preset_name),
    };

    registry.spawner.spawn(task, Some(resources)).await?;
    Ok(())
}

/// Generate a task ID for a registry-spawned task.
fn generate_registry_task_id(preset_name: &str) -> String {
    let timestamp_ms = chrono::Utc::now().timestamp_millis();
    let rand_part: u32 = rand::thread_rng().r#gen();
    format!("{preset_name}-{rand_part:08x}-{timestamp_ms}")
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
}
