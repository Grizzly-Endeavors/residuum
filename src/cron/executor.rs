//! Cron job execution: runs due jobs and updates their state.

use chrono::NaiveDateTime;
use chrono_tz::Tz;

use crate::agent::Agent;
use crate::agent::context::{ProjectsContext, SkillsContext};
use crate::channels::null::NullDisplay;
use crate::error::IronclawError;
use crate::models::ModelProvider;

use super::scheduler::compute_next_run_with_backoff;
use super::store::CronStore;
use super::types::{CronJob, CronPayload, RunStatus};

/// Result of executing due cron jobs.
pub struct CronExecutionResult {
    /// Per-job results for routing.
    pub results: Vec<CronJobResult>,
    /// Aggregate messages for the memory pipeline.
    pub messages: Vec<crate::models::Message>,
}

/// Result of executing a single cron job.
pub struct CronJobResult {
    /// Name of the job that was executed.
    pub job_name: String,
    /// Messages from this job for the memory pipeline.
    pub messages: Vec<crate::models::Message>,
    /// Summary text for routing (None if job produced no summary).
    pub summary: Option<String>,
}

/// Execute all due cron jobs.
///
/// If `provider_override` is `Some`, that provider is used for any `AgentTurn`
/// jobs instead of the agent's default provider.
///
/// # Errors
/// Returns `IronclawError` if a store save fails. Individual job failures are
/// recorded in the job's state and do not abort the loop.
pub async fn execute_due_jobs(
    store: &mut CronStore,
    agent: &mut Agent,
    now: NaiveDateTime,
    tz: Tz,
    provider_override: Option<&dyn ModelProvider>,
    projects_ctx: &ProjectsContext<'_>,
) -> Result<CronExecutionResult, IronclawError> {
    let due_ids: Vec<String> = store
        .find_due_jobs(now)
        .iter()
        .map(|j| j.id.clone())
        .collect();

    let mut all_messages: Vec<crate::models::Message> = Vec::new();
    let mut job_results: Vec<CronJobResult> = Vec::new();

    for job_id in due_ids {
        // Clone job to avoid borrow conflict with store
        let Some(job) = store.get_job(&job_id).cloned() else {
            continue;
        };

        let (status, error_msg, new_messages, summary) =
            run_job(&job, agent, provider_override, projects_ctx).await;

        job_results.push(CronJobResult {
            job_name: job.name.clone(),
            messages: new_messages.clone(),
            summary,
        });

        // Update job state
        let Some(job_mut) = store.get_job_mut(&job_id) else {
            continue;
        };

        job_mut.state.last_run_at = Some(now);
        job_mut.state.last_status = Some(status);

        match status {
            RunStatus::Ok => {
                job_mut.state.consecutive_errors = 0;
                job_mut.state.last_error = None;

                // One-shot At job: disable (and delete if requested)
                if matches!(job_mut.schedule, super::types::CronSchedule::At { .. }) {
                    job_mut.enabled = false;
                }
            }
            RunStatus::Error => {
                job_mut.state.consecutive_errors =
                    job_mut.state.consecutive_errors.saturating_add(1);
                job_mut.state.last_error = error_msg;
            }
            RunStatus::Skipped => {}
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

        all_messages.extend(new_messages);
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

    store.save().await?;
    Ok(CronExecutionResult {
        results: job_results,
        messages: all_messages,
    })
}

/// Run a single cron job, returning status, error, messages, and an optional summary.
async fn run_job(
    job: &CronJob,
    agent: &mut Agent,
    provider_override: Option<&dyn ModelProvider>,
    projects_ctx: &ProjectsContext<'_>,
) -> (
    RunStatus,
    Option<String>,
    Vec<crate::models::Message>,
    Option<String>,
) {
    match &job.payload {
        CronPayload::SystemEvent { text } => {
            tracing::info!(job = %job.name, "system event: {}", text);
            let msg = crate::models::Message::user(format!("[cron: {}] {}", job.name, text));
            (RunStatus::Ok, None, vec![msg], Some(text.clone()))
        }

        CronPayload::AgentTurn { message } => {
            let display = NullDisplay;
            match agent
                .run_system_turn(
                    message,
                    &display,
                    provider_override,
                    projects_ctx,
                    &SkillsContext::none(),
                )
                .await
            {
                Ok(result) => {
                    tracing::info!(job = %job.name, "agent turn completed");
                    (RunStatus::Ok, None, result.messages, Some(result.response))
                }
                Err(e) => {
                    tracing::warn!(job = %job.name, error = %e, "agent turn failed");
                    (RunStatus::Error, Some(e.to_string()), Vec::new(), None)
                }
            }
        }
    }
}
