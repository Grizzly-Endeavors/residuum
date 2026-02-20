//! Cron job execution: runs due jobs and updates their state.

use chrono::{DateTime, Utc};

use crate::agent::Agent;
use crate::channels::null::NullDisplay;
use crate::error::IronclawError;

use super::scheduler::compute_next_run_with_backoff;
use super::store::CronStore;
use super::types::{CronJob, CronPayload, RunStatus, SessionTarget};

/// Execute all due cron jobs.
///
/// For each due job:
/// - `SystemEvent + Main`: queues the event text on the agent
/// - `AgentTurn + Isolated`: runs `agent.run_system_turn` and returns messages
///
/// Returns messages from isolated agent turns for the memory pipeline.
/// Main-session system events are queued directly on the agent.
///
/// # Errors
/// Returns `IronclawError` if a store save fails. Individual job failures are
/// recorded in the job's state and do not abort the loop.
pub async fn execute_due_jobs(
    store: &mut CronStore,
    agent: &mut Agent,
    now: DateTime<Utc>,
) -> Result<Vec<crate::models::Message>, IronclawError> {
    let due_ids: Vec<String> = store
        .find_due_jobs(now)
        .iter()
        .map(|j| j.id.clone())
        .collect();

    let mut all_messages: Vec<crate::models::Message> = Vec::new();

    for job_id in due_ids {
        // Clone job to avoid borrow conflict with store
        let Some(job) = store.get_job(&job_id).cloned() else {
            continue;
        };

        let (status, error_msg, new_messages) = run_job(&job, agent).await;

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
        match compute_next_run_with_backoff(job_mut, now) {
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
    Ok(all_messages)
}

/// Run a single cron job. Returns (status, `error_message`, `ephemeral_messages`).
async fn run_job(
    job: &CronJob,
    agent: &mut Agent,
) -> (RunStatus, Option<String>, Vec<crate::models::Message>) {
    match (&job.payload, job.session_target) {
        (CronPayload::SystemEvent { text }, SessionTarget::Main) => {
            // Announce to CLI
            println!("\n[cron: {}] {}\n", job.name, text);
            agent.queue_system_event(text.clone());
            (RunStatus::Ok, None, Vec::new())
        }

        (CronPayload::SystemEvent { text }, SessionTarget::Isolated) => {
            // Isolated system event: just log it
            tracing::info!(job = %job.name, "isolated system event: {}", text);
            (RunStatus::Ok, None, Vec::new())
        }

        (CronPayload::AgentTurn { message }, _) => {
            let display = NullDisplay;
            match agent.run_system_turn(message, &display).await {
                Ok(result) => {
                    tracing::info!(job = %job.name, "agent turn completed");
                    (RunStatus::Ok, None, result.messages)
                }
                Err(e) => {
                    eprintln!("warning: cron job '{}' agent turn failed: {e}", job.name);
                    tracing::warn!(job = %job.name, error = %e, "agent turn failed");
                    (RunStatus::Error, Some(e.to_string()), Vec::new())
                }
            }
        }
    }
}

/// Initialize `next_run_at` for a newly created job.
///
/// Should be called after adding a job to the store to set the first fire time.
///
/// # Errors
/// Returns `IronclawError::Scheduling` if the schedule cannot be parsed.
pub fn initialize_next_run(job: &mut CronJob, now: DateTime<Utc>) -> Result<(), IronclawError> {
    let next = compute_next_run_with_backoff(job, now)?;
    job.state.next_run_at = next;
    Ok(())
}
