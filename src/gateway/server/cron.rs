//! Cron job execution helper for the gateway.

use std::sync::Arc;

use tokio::sync::broadcast;

use crate::agent::Agent;
use crate::agent::context::ProjectsContext;
use crate::cron::executor::execute_due_jobs;
use crate::cron::store::CronStore;
use crate::gateway::protocol::ServerMessage;
use crate::memory::observer::{ObserveAction, Observer};
use crate::memory::types::Visibility;
use crate::models::ModelProvider;
use crate::projects::activation::SharedProjectState;
use crate::workspace::layout::WorkspaceLayout;

use super::context::{build_project_context_strings, project_context_label};
use super::memory::persist_and_check_thresholds;

/// Execute due cron jobs, broadcast notifications, and persist messages.
///
/// Returns the `ObserveAction` so the caller can manage the observe deadline.
#[expect(
    clippy::too_many_arguments,
    reason = "gateway helper wiring multiple subsystems together"
)]
pub(super) async fn run_due_cron_jobs_gateway(
    cron_store: &Arc<tokio::sync::Mutex<CronStore>>,
    agent: &mut Agent,
    observer: &Observer,
    layout: &WorkspaceLayout,
    broadcast_tx: &broadcast::Sender<ServerMessage>,
    provider_override: Option<&dyn ModelProvider>,
    tz: chrono_tz::Tz,
    project_state: &SharedProjectState,
) -> ObserveAction {
    let now = crate::time::now_local(tz);
    let mut store = cron_store.lock().await;

    // Reload from disk so external edits to jobs.json take effect immediately
    match CronStore::load(layout.cron_jobs_json()).await {
        Ok(fresh) => *store = fresh,
        Err(e) => {
            tracing::warn!(error = %e, "failed to reload cron store from disk; using in-memory state");
        }
    }

    let (cron_idx_text, cron_active_text) = build_project_context_strings(project_state).await;
    let cron_projects_ctx = ProjectsContext {
        index: cron_idx_text.as_deref(),
        active_context: cron_active_text.as_deref(),
    };

    match execute_due_jobs(
        &mut store,
        agent,
        now,
        tz,
        provider_override,
        &cron_projects_ctx,
    )
    .await
    {
        Ok(result) => {
            for notif in &result.notifications {
                if broadcast_tx
                    .send(ServerMessage::SystemEvent {
                        source: format!("cron: {}", notif.job_name),
                        content: notif.text.clone(),
                    })
                    .is_err()
                {
                    tracing::trace!("no broadcast receivers for cron notification");
                }
            }
            if !result.messages.is_empty() {
                let cron_context = project_context_label(project_state, layout).await;
                return persist_and_check_thresholds(
                    &result.messages,
                    &cron_context,
                    Visibility::Background,
                    observer,
                    layout,
                    tz,
                )
                .await;
            }
            ObserveAction::None
        }
        Err(e) => {
            tracing::warn!(error = %e, "cron execution failed");
            ObserveAction::None
        }
    }
}
