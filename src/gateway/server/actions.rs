//! Scheduled action execution helper for the gateway.

use std::sync::Arc;

use crate::actions::store::ActionStore;
use crate::actions::types::ScheduledAction;
use crate::background::BackgroundTaskSpawner;
use crate::background::spawn_context::load_preset_for_spawn;
use crate::background::types::{BackgroundTask, Execution, ResultRouting, SubAgentConfig};
use crate::config::BackgroundModelTier;
use crate::mcp::SharedMcpRegistry;
use crate::notify::types::TaskSource;
use crate::projects::activation::SharedProjectState;
use crate::skills::SharedSkillState;
use crate::workspace::layout::WorkspaceLayout;

use super::spawn_helpers::{SpawnContext, build_spawn_resources};

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
/// - `agent = Some(preset)` actions are spawned with the named preset.
/// - `agent = None` actions are spawned at `Medium` tier with `ResultRouting::Direct`.
///
/// Due actions are drained from the store and saved.
pub(super) async fn spawn_due_actions(
    action_store: &Arc<tokio::sync::Mutex<ActionStore>>,
    layout: &WorkspaceLayout,
    spawn_ctx: &SpawnContext,
    project_state: &SharedProjectState,
    skill_state: &SharedSkillState,
    mcp_registry: &SharedMcpRegistry,
    spawner: &Arc<BackgroundTaskSpawner>,
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
                spawn_action_with_preset(
                    action,
                    preset_name,
                    &layout.subagents_dir(),
                    spawn_ctx,
                    project_state,
                    skill_state,
                    mcp_registry,
                    spawner,
                )
                .await;
            }
            None => {
                spawn_action_default(
                    action,
                    spawn_ctx,
                    project_state,
                    skill_state,
                    mcp_registry,
                    spawner,
                )
                .await;
            }
        }
    }

    if let Err(e) = store.save().await {
        tracing::warn!(error = %e, "failed to save action store after spawning due actions");
    }

    main_turns
}

/// Spawn an action with the default Medium tier and no preset.
async fn spawn_action_default(
    action: &ScheduledAction,
    spawn_ctx: &SpawnContext,
    project_state: &SharedProjectState,
    skill_state: &SharedSkillState,
    mcp_registry: &SharedMcpRegistry,
    spawner: &Arc<BackgroundTaskSpawner>,
) {
    let tier = action
        .model_tier
        .as_deref()
        .and_then(|s| s.parse().ok())
        .unwrap_or(BackgroundModelTier::Medium);

    let task = BackgroundTask {
        id: format!(
            "action-{}-{}",
            action.id,
            chrono::Utc::now().timestamp_millis()
        ),
        task_name: action.name.clone(),
        source: TaskSource::Action,
        execution: Execution::SubAgent(SubAgentConfig {
            prompt: action.prompt.clone(),
            context: None,
            model_tier: tier,
        }),
        routing: ResultRouting::Direct(action.channels.clone()),
    };

    match build_spawn_resources(
        spawn_ctx,
        &tier,
        project_state,
        skill_state,
        Arc::clone(mcp_registry),
        None,
    )
    .await
    {
        Ok(resources) => {
            if let Err(e) = spawner.spawn(task, Some(resources)).await {
                tracing::warn!(action = %action.name, error = %e, "failed to spawn action task");
            }
        }
        Err(e) => {
            tracing::warn!(action = %action.name, error = %e, "failed to build action resources");
        }
    }
}

/// Spawn an action using a named sub-agent preset.
#[expect(
    clippy::too_many_arguments,
    reason = "passes through subsystem references for resource construction"
)]
async fn spawn_action_with_preset(
    action: &ScheduledAction,
    preset_name: &str,
    subagents_dir: &std::path::Path,
    spawn_ctx: &SpawnContext,
    project_state: &SharedProjectState,
    skill_state: &SharedSkillState,
    mcp_registry: &SharedMcpRegistry,
    spawner: &Arc<BackgroundTaskSpawner>,
) {
    let tier_fallback = action
        .model_tier
        .as_deref()
        .and_then(|s| s.parse().ok())
        .unwrap_or(BackgroundModelTier::Small);

    let (tier, preset) =
        match load_preset_for_spawn(subagents_dir, preset_name, tier_fallback).await {
            Ok(result) => result,
            Err(e) => {
                tracing::warn!(
                    action = %action.name,
                    preset = preset_name,
                    error = %e,
                    "failed to load preset for action, falling back to default"
                );
                (BackgroundModelTier::Medium, None)
            }
        };

    let task = BackgroundTask {
        id: format!(
            "action-{}-{}",
            action.id,
            chrono::Utc::now().timestamp_millis()
        ),
        task_name: action.name.clone(),
        source: TaskSource::Action,
        execution: Execution::SubAgent(SubAgentConfig {
            prompt: action.prompt.clone(),
            context: None,
            model_tier: tier,
        }),
        routing: ResultRouting::Direct(action.channels.clone()),
    };

    let preset_arg = preset.as_ref().map(|(fm, body)| (fm, body.clone()));

    match build_spawn_resources(
        spawn_ctx,
        &tier,
        project_state,
        skill_state,
        Arc::clone(mcp_registry),
        preset_arg,
    )
    .await
    {
        Ok(resources) => {
            if let Err(e) = spawner.spawn(task, Some(resources)).await {
                tracing::warn!(action = %action.name, error = %e, "failed to spawn action task with preset");
            }
        }
        Err(e) => {
            tracing::warn!(action = %action.name, error = %e, "failed to build action resources with preset");
        }
    }
}
