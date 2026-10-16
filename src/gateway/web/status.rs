//! The running agent's status route (`GET /api/agents/{name}/status`).

use axum::extract::State;
use axum::response::Json;
use serde::Serialize;

use crate::features;

use super::ConfigApiState;

/// Status response: which mode the server is running in, its version, and the
/// feature ids it supports. Workbench artifacts read the same information
/// from the injected SDK's `residuum.version` and `residuum.features`.
#[derive(Serialize)]
pub(super) struct StatusResponse {
    mode: &'static str,
    version: &'static str,
    features: &'static [&'static str],
    /// On-disk size and checkpoint count for each checkpoint repository, so
    /// growth is visible.
    /// `null` for a repo whose stats couldn't be read just now.
    checkpoints: CheckpointsStatus,
    /// Whether file changes in the agent's directory reach open pages and
    /// the agent's watchers: `starting`, `native` (OS notifications),
    /// `polling` (the fallback, up to two seconds late) or `off`.
    live_updates: crate::workspace::watch::WatchHealth,
}

/// What the running agent's status route reads: its config state and the
/// health of the change feed over its directory.
#[derive(Clone)]
pub(crate) struct AgentStatusState {
    pub(crate) build: crate::BuildInfo,
    pub(crate) config: ConfigApiState,
    pub(crate) live_updates: tokio::sync::watch::Receiver<crate::workspace::watch::WatchHealth>,
}

/// Per-repository checkpoint stats shown in `/api/agents/{name}/status`.
#[derive(Serialize)]
pub(super) struct CheckpointsStatus {
    /// How many of the agent's automatic checkpoints are still being
    /// written; they run off the turn's path.
    pending: usize,
    workspace: Option<crate::checkpoints::RepoStats>,
    team: Option<crate::checkpoints::RepoStats>,
    agent_config: Option<crate::checkpoints::RepoStats>,
    hub: Option<crate::checkpoints::RepoStats>,
}

/// `GET /api/agents/{name}/status` — returns `{ mode, version, features, checkpoints }`.
pub(super) async fn api_status(
    State(AgentStatusState {
        build,
        config: state,
        live_updates,
    }): State<AgentStatusState>,
) -> Json<StatusResponse> {
    let live_updates = *live_updates.borrow();
    Json(StatusResponse {
        live_updates,
        mode: "running",
        version: build.version,
        features: features::FEATURES,
        checkpoints: CheckpointsStatus {
            pending: state.checkpoints.pending_checkpoints(),
            workspace: checkpoint_stats_or_log(&state, crate::checkpoints::RepoKind::Workspace)
                .await,
            team: checkpoint_stats_or_log(&state, crate::checkpoints::RepoKind::Team).await,
            agent_config: checkpoint_stats_or_log(
                &state,
                crate::checkpoints::RepoKind::AgentConfig,
            )
            .await,
            hub: checkpoint_stats_or_log(&state, crate::checkpoints::RepoKind::Hub).await,
        },
    })
}

/// A repo's checkpoint stats, or `None` (logged) if they couldn't be read —
/// `/api/agents/{name}/status` degrades rather than failing over a checkpoint read.
async fn checkpoint_stats_or_log(
    state: &ConfigApiState,
    kind: crate::checkpoints::RepoKind,
) -> Option<crate::checkpoints::RepoStats> {
    state.checkpoints.stats(kind).await.map_or_else(
        |e| {
            tracing::warn!(error = %e, ?kind, "failed to read checkpoint stats for /api/status");
            None
        },
        Some,
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn state() -> ConfigApiState {
        ConfigApiState {
            team: None,
            hub_dir: PathBuf::from("/tmp/residuum-test-nonexistent-hub"),
            config_dir: PathBuf::from("/tmp/residuum-test-nonexistent"),
            agent_name: "test-agent".to_string(),
            workspace_dir: PathBuf::from("/tmp/residuum-test-nonexistent"),
            memory_dir: None,
            reload_tx: None,
            scope: crate::gateway::web::WorkspaceScope::Agent,
            checkpoints: crate::checkpoints::test_engine(),
        }
    }

    fn status_state(
        live_updates: crate::workspace::watch::WatchHealth,
    ) -> (
        AgentStatusState,
        tokio::sync::watch::Sender<crate::workspace::watch::WatchHealth>,
    ) {
        let (tx, rx) = tokio::sync::watch::channel(live_updates);
        (
            AgentStatusState {
                build: crate::BuildInfo {
                    version: "1.2.3",
                    commit: None,
                },
                config: state(),
                live_updates: rx,
            },
            tx,
        )
    }

    #[tokio::test]
    async fn status_reports_mode_version_and_features() {
        let (state, _tx) = status_state(crate::workspace::watch::WatchHealth::Native);
        let Json(running) = api_status(State(state)).await;
        assert_eq!(running.mode, "running");
        assert_eq!(running.version, "1.2.3");
        assert_eq!(running.features, features::FEATURES);
    }

    #[tokio::test]
    async fn status_reports_how_live_updates_reach_the_agent() {
        use crate::workspace::watch::WatchHealth;

        let (state, tx) = status_state(WatchHealth::Starting);
        let read = |current: AgentStatusState| async move {
            let Json(status) = api_status(State(current)).await;
            serde_json::to_value(&status)
                .unwrap()
                .get("live_updates")
                .cloned()
                .unwrap()
        };
        assert_eq!(read(state.clone()).await, "starting");
        tx.send_replace(WatchHealth::Polling);
        assert_eq!(
            read(state.clone()).await,
            "polling",
            "a fallback to polling shows in the status"
        );
        tx.send_replace(WatchHealth::Off);
        assert_eq!(read(state).await, "off");
    }
}
