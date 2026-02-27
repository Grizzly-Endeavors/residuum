//! Cron job spawning helper for the gateway.

use std::path::Path;
use std::sync::Arc;

use crate::background::BackgroundTaskSpawner;
use crate::background::types::{
    BackgroundResult, BackgroundTask, Execution, ResultRouting, SubAgentConfig, TaskStatus,
};
use crate::config::BackgroundModelTier;
use crate::cron::scheduler::compute_next_run_with_backoff;
use crate::cron::store::CronStore;
use crate::cron::types::{CronPayload, RunStatus};
use crate::mcp::SharedMcpRegistry;
use crate::notify::types::TaskSource;
use crate::projects::activation::SharedProjectState;
use crate::skills::SharedSkillState;
use crate::workspace::layout::WorkspaceLayout;

use super::spawn_helpers::{SpawnContext, build_spawn_resources};

/// A cron job that should run as a main agent wake turn rather than a sub-agent.
pub(super) struct CronMainTurn {
    /// Job name (for logging/formatting).
    pub job_name: String,
    /// The prompt to inject.
    pub prompt: String,
}

/// Spawn due cron jobs as background tasks, returning any that need main agent turns.
///
/// - `SystemEvent` jobs are sent directly as `BackgroundResult` (no LLM call).
/// - `AgentTurn { agent: Some("main") }` jobs are returned as `CronMainTurn`s.
/// - `AgentTurn { agent: Some(preset) }` jobs are spawned with the named preset.
/// - `AgentTurn { agent: None }` jobs are spawned at `Medium` tier (existing behavior).
///
/// Job state (`last_run_at`, `next_run_at`, etc.) is updated optimistically and saved.
#[expect(
    clippy::too_many_arguments,
    reason = "gateway helper wiring multiple subsystems together"
)]
#[expect(
    clippy::too_many_lines,
    reason = "sequential spawn + state-update loop for each due job; splitting would fragment the transactional logic"
)]
pub(super) async fn spawn_due_cron_jobs(
    cron_store: &Arc<tokio::sync::Mutex<CronStore>>,
    layout: &WorkspaceLayout,
    spawn_ctx: &SpawnContext,
    project_state: &SharedProjectState,
    skill_state: &SharedSkillState,
    mcp_registry: &SharedMcpRegistry,
    spawner: &Arc<BackgroundTaskSpawner>,
    tz: chrono_tz::Tz,
) -> Vec<CronMainTurn> {
    let now = crate::time::now_local(tz);
    let mut store = cron_store.lock().await;
    let mut main_turns = Vec::new();

    // Reload from disk so external edits to jobs.json take effect immediately
    match CronStore::load(layout.cron_jobs_json()).await {
        Ok(fresh) => *store = fresh,
        Err(e) => {
            tracing::warn!(error = %e, "failed to reload cron store from disk; using in-memory state");
        }
    }

    let due_ids: Vec<String> = store
        .find_due_jobs(now)
        .iter()
        .map(|j| j.id.clone())
        .collect();

    for job_id in &due_ids {
        let Some(job) = store.get_job(job_id).cloned() else {
            continue;
        };

        let timestamp_ms = chrono::Utc::now().timestamp_millis();
        let spawn_ok = match &job.payload {
            CronPayload::SystemEvent { text } => {
                tracing::info!(job = %job.name, "cron system event: {text}");
                let result = BackgroundResult {
                    id: format!("cron-evt-{job_id}-{timestamp_ms}"),
                    task_name: job.name.clone(),
                    source: TaskSource::Cron,
                    summary: text.clone(),
                    transcript_path: None,
                    status: TaskStatus::Completed,
                    timestamp: chrono::Utc::now(),
                    routing: ResultRouting::Notify,
                };
                if let Err(e) = spawner.send_result(result).await {
                    tracing::warn!(job = %job.name, error = %e, "failed to send cron system event result");
                    false
                } else {
                    true
                }
            }
            CronPayload::AgentTurn { message, agent } => match agent.as_deref() {
                Some("main") => {
                    main_turns.push(CronMainTurn {
                        job_name: job.name.clone(),
                        prompt: message.clone(),
                    });
                    true
                }
                Some(preset_name) => {
                    spawn_cron_agent_with_preset(
                        &job.name,
                        job_id,
                        timestamp_ms,
                        message,
                        preset_name,
                        &layout.subagents_dir(),
                        spawn_ctx,
                        project_state,
                        skill_state,
                        mcp_registry,
                        spawner,
                    )
                    .await
                }
                None => {
                    spawn_cron_agent_default(
                        &job.name,
                        job_id,
                        timestamp_ms,
                        message,
                        spawn_ctx,
                        project_state,
                        skill_state,
                        mcp_registry,
                        spawner,
                    )
                    .await
                }
            },
        };

        // Update job state optimistically
        let Some(job_mut) = store.get_job_mut(job_id) else {
            continue;
        };

        job_mut.state.last_run_at = Some(now);

        if spawn_ok {
            job_mut.state.last_status = Some(RunStatus::Ok);
            job_mut.state.consecutive_errors = 0;
            job_mut.state.last_error = None;

            // One-shot At job: disable after firing
            if matches!(
                job_mut.schedule,
                crate::cron::types::CronSchedule::At { .. }
            ) {
                job_mut.enabled = false;
            }
        } else {
            job_mut.state.last_status = Some(RunStatus::Error);
            job_mut.state.consecutive_errors = job_mut.state.consecutive_errors.saturating_add(1);
            job_mut.state.last_error = Some("failed to spawn background task".to_string());
        }

        // Recompute next_run with backoff applied
        match compute_next_run_with_backoff(job_mut, now, tz) {
            Ok(next) => job_mut.state.next_run_at = next,
            Err(e) => {
                eprintln!("warning: cron job '{job_id}' schedule is invalid, disabling: {e}");
                tracing::warn!(job = %job_id, error = %e, "failed to compute next_run_at, disabling job");
                job_mut.enabled = false;
            }
        }
    }

    // Remove jobs marked for deletion after a successful one-shot run
    let to_delete: Vec<String> = store
        .list_jobs()
        .iter()
        .filter(|j| j.delete_after_run && j.state.last_status == Some(RunStatus::Ok) && !j.enabled)
        .map(|j| j.id.clone())
        .collect();

    for id in to_delete {
        store.remove_job(&id);
    }

    if let Err(e) = store.save().await {
        tracing::warn!(error = %e, "failed to save cron store after spawning due jobs");
    }

    main_turns
}

/// Spawn a cron agent turn with the default Medium tier and no preset.
#[expect(
    clippy::too_many_arguments,
    reason = "passes through subsystem references for resource construction"
)]
async fn spawn_cron_agent_default(
    job_name: &str,
    job_id: &str,
    timestamp_ms: i64,
    message: &str,
    spawn_ctx: &SpawnContext,
    project_state: &SharedProjectState,
    skill_state: &SharedSkillState,
    mcp_registry: &SharedMcpRegistry,
    spawner: &Arc<BackgroundTaskSpawner>,
) -> bool {
    let task = BackgroundTask {
        id: format!("cron-agent-{job_id}-{timestamp_ms}"),
        task_name: job_name.to_string(),
        source: TaskSource::Cron,
        execution: Execution::SubAgent(SubAgentConfig {
            prompt: message.to_string(),
            context: None,
            model_tier: BackgroundModelTier::Medium,
        }),
        routing: ResultRouting::Notify,
    };
    match build_spawn_resources(
        spawn_ctx,
        &BackgroundModelTier::Medium,
        project_state,
        skill_state,
        Arc::clone(mcp_registry),
        None,
    )
    .await
    {
        Ok(resources) => {
            if let Err(e) = spawner.spawn(task, Some(resources)).await {
                tracing::warn!(job = %job_name, error = %e, "failed to spawn cron agent task");
                false
            } else {
                true
            }
        }
        Err(e) => {
            tracing::warn!(job = %job_name, error = %e, "failed to build cron resources");
            false
        }
    }
}

/// Spawn a cron agent turn using a named sub-agent preset.
#[expect(
    clippy::too_many_arguments,
    reason = "passes through subsystem references for resource construction"
)]
async fn spawn_cron_agent_with_preset(
    job_name: &str,
    job_id: &str,
    timestamp_ms: i64,
    message: &str,
    preset_name: &str,
    subagents_dir: &Path,
    spawn_ctx: &SpawnContext,
    project_state: &SharedProjectState,
    skill_state: &SharedSkillState,
    mcp_registry: &SharedMcpRegistry,
    spawner: &Arc<BackgroundTaskSpawner>,
) -> bool {
    let (tier, preset) =
        match load_preset_for_spawn(subagents_dir, preset_name, BackgroundModelTier::Small).await {
            Ok(result) => result,
            Err(e) => {
                tracing::warn!(
                    job = %job_name,
                    preset = preset_name,
                    error = %e,
                    "failed to load preset for cron job, falling back to default"
                );
                (BackgroundModelTier::Medium, None)
            }
        };

    let task = BackgroundTask {
        id: format!("cron-agent-{job_id}-{timestamp_ms}"),
        task_name: job_name.to_string(),
        source: TaskSource::Cron,
        execution: Execution::SubAgent(SubAgentConfig {
            prompt: message.to_string(),
            context: None,
            model_tier: tier,
        }),
        routing: ResultRouting::Notify,
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
                tracing::warn!(job = %job_name, error = %e, "failed to spawn cron agent task with preset");
                false
            } else {
                true
            }
        }
        Err(e) => {
            tracing::warn!(job = %job_name, error = %e, "failed to build cron resources with preset");
            false
        }
    }
}

/// Load a sub-agent preset and resolve its model tier.
///
/// Returns the resolved tier and the preset frontmatter+body (if loaded successfully).
/// Used by both pulse and cron code paths.
pub(super) async fn load_preset_for_spawn(
    subagents_dir: &Path,
    preset_name: &str,
    fallback_tier: BackgroundModelTier,
) -> Result<
    (
        BackgroundModelTier,
        Option<(crate::subagents::types::SubagentPresetFrontmatter, String)>,
    ),
    anyhow::Error,
> {
    let index = crate::subagents::SubagentPresetIndex::scan(subagents_dir).await?;
    let (fm, body) = index.load_preset(preset_name).await?;

    let tier: BackgroundModelTier = fm
        .model_tier
        .as_deref()
        .and_then(|s| s.parse().ok())
        .unwrap_or(fallback_tier);

    Ok((tier, Some((fm, body))))
}
