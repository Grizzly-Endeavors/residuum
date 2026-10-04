//! Web UI static asset serving and config API endpoints.
//!
//! Embeds the `web/dist/` directory into the binary via `rust-embed`.
//! In debug builds, files are served from disk (hot-reload); in release
//! builds, they are compiled into the binary.

use std::path::PathBuf;
use std::sync::Arc;

use axum::routing::{delete, get, patch, post, put};
use tokio::sync::watch;

pub(crate) mod a2a;
mod agent_files;
mod agent_keys;
pub(crate) mod artifact_identity;
mod assets;
pub(super) mod chat;
pub mod checkpoints;
pub mod cloud;
pub mod config;
pub mod inbox;
pub(crate) mod memory;
pub(crate) mod model;
pub mod providers;
pub(crate) mod scheduled;
pub mod secrets;
pub(crate) mod sessions;
pub mod teams_setup;
pub mod tracing_api;
pub mod update;
pub(crate) mod workbench;
pub mod workspace;
pub(crate) mod workspace_bulk;
#[cfg(test)]
mod workspace_team_tests;

mod embedded {
    //! Module boundary isolates `rust-embed` derive from clippy `same_name_method`.
    #![expect(
        clippy::same_name_method,
        reason = "rust-embed derive generates get/iter methods that shadow trait methods"
    )]

    use rust_embed::Embed;

    /// Embedded web assets from `web/dist/`.
    #[derive(Embed)]
    #[folder = "web/dist/"]
    pub(super) struct WebAssets;
}
pub(crate) use assets::static_assets;
use embedded::WebAssets;

pub use agent_files::{AgentFilesState, CheckpointAccess, agent_files_api_router};

/// Shared state for the config API.
#[derive(Clone)]
pub struct ConfigApiState {
    /// Path to the hub directory (`~/.residuum/hub`): secrets, key stores,
    /// hub `config.toml`.
    pub hub_dir: PathBuf,
    /// Path to the agent's own `config/` directory (`config.toml`,
    /// `providers.toml`, `mcp.json`, `channels.toml`, ...).
    pub config_dir: PathBuf,
    /// This agent's name.
    pub agent_name: String,
    /// Path to the workspace root directory (for resolving `mcp.json`, `channels.toml`, etc.).
    pub workspace_dir: PathBuf,
    /// Path to the workspace memory directory (None in setup mode).
    pub memory_dir: Option<PathBuf>,
    /// Signal the running agent to reload (None when there is no live agent
    /// to signal, as on a stopped agent's repair routes).
    pub reload_tx: Option<crate::gateway::types::ReloadSender>,
    /// Workspace and config checkpoint repositories.
    pub checkpoints: Arc<crate::checkpoints::CheckpointEngine>,
    /// The `team/` namespace and write coordination for the workspace file
    /// API, with writes attributed to the user. `None` where `team/` is an
    /// ordinary name.
    pub team: Option<crate::workspace::team_files::TeamFiles>,
    /// Which namespace the workspace file routes address.
    pub scope: WorkspaceScope,
}

/// The namespace a workspace file API addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceScope {
    /// An agent's directory, with the team directory reachable as `team/...`.
    Agent,
    /// The team directory alone: every path is relative to `team/`.
    Team,
}

/// Shared state for the hub-level API: everything that exists once per
/// process rather than once per agent.
#[derive(Clone)]
pub(crate) struct HubApiState {
    /// Path to the hub directory (`~/.residuum/hub`): secrets, key stores,
    /// hub `config.toml`.
    pub hub_dir: PathBuf,
    /// Signals the hub to reload after a hub-owned file changes.
    pub reload_tx: crate::gateway::types::ReloadSender,
    /// Signalled once onboarding has written the first agent (None once the
    /// hub has agents and setup can no longer run).
    pub setup_done: Option<Arc<watch::Sender<bool>>>,
    /// Serializes secret store writes to prevent lost-update races.
    pub secret_lock: Arc<tokio::sync::Mutex<()>>,
    /// Checkpoint repositories; the hub API addresses the hub and team ones.
    pub checkpoints: Arc<crate::checkpoints::CheckpointEngine>,
    /// Coordinates writes under the team directory, which onboarding
    /// bootstraps.
    pub team: crate::workspace::team_files::TeamWriteCoordinator,
}

/// The hub directory, for routes that only need to resolve `secret:` values
/// and so serve under both the hub and an agent.
#[derive(Clone)]
pub(crate) struct HubDir(pub PathBuf);

impl axum::extract::FromRef<HubApiState> for HubDir {
    fn from_ref(state: &HubApiState) -> Self {
        Self(state.hub_dir.clone())
    }
}

impl axum::extract::FromRef<ConfigApiState> for HubDir {
    fn from_ref(state: &ConfigApiState) -> Self {
        Self(state.hub_dir.clone())
    }
}

impl HubApiState {
    /// A state over `hub_dir` with throwaway checkpoint repositories and no
    /// one listening for reloads or setup completion.
    #[cfg(test)]
    pub(crate) fn for_test(hub_dir: &std::path::Path) -> Self {
        let (reload_tx, _reload_rx) = tokio::sync::mpsc::unbounded_channel();
        Self {
            hub_dir: hub_dir.to_path_buf(),
            reload_tx,
            setup_done: None,
            secret_lock: Arc::new(tokio::sync::Mutex::new(())),
            checkpoints: crate::checkpoints::test_engine(),
            team: crate::workspace::team_files::TeamWriteCoordinator::new(
                &crate::config::paths::TeamPaths::new(hub_dir.join("team-for-test")),
            ),
        }
    }

    /// Bootstrap `layout`'s workspace and the team directory under the hub's
    /// team write coordinator.
    pub(super) async fn bootstrap_workspace(
        &self,
        layout: &crate::workspace::layout::WorkspaceLayout,
        user_name: Option<&str>,
        timezone: &str,
        label: Option<&str>,
    ) -> Result<(), crate::util::FatalError> {
        crate::workspace::bootstrap::ensure_workspace_labeled(
            layout,
            &self.team,
            user_name,
            Some(timezone),
            label,
        )
        .await
    }

    /// Checkpoint the hub config repository (hub `config.toml` and the
    /// encrypted key stores) before a write to one of them. Never fails or
    /// blocks the write — see `crate::checkpoints`.
    pub(super) async fn checkpoint_config_before_write(&self, summary: impl Into<String>) {
        let _checkpoint_id = self.checkpoint_config_id_before_write(summary).await;
    }

    /// [`Self::checkpoint_config_before_write`], returning the id of the
    /// checkpoint that holds the pre-write tree. `None` when that checkpoint
    /// could not be recorded; the write still proceeds.
    #[must_use]
    pub(super) async fn checkpoint_config_id_before_write(
        &self,
        summary: impl Into<String>,
    ) -> Option<String> {
        self.checkpoints
            .checkpoint_config_id_before_write(crate::checkpoints::CheckpointContext::system(
                crate::checkpoints::CheckpointTrigger::PreConfigWrite,
                summary,
            ))
            .await
    }
}

impl ConfigApiState {
    /// The team directory when it appears inside this state's namespace as
    /// the `team/` folder. `None` in the team scope, where the namespace is
    /// the team directory itself, and where `team/` is an ordinary name.
    fn team_mount(&self) -> Option<PathBuf> {
        match self.scope {
            WorkspaceScope::Agent => self.team.as_ref().map(|t| t.team_root().to_path_buf()),
            WorkspaceScope::Team => None,
        }
    }

    /// The paths the strictly-parsed-file diagnostics resolve against. The
    /// team scope has no agent config to recognize, so both point at the hub
    /// directory, where no team file lives.
    fn diagnostics_paths(&self) -> crate::diagnostics::DiagnosticsPaths {
        match self.scope {
            WorkspaceScope::Agent => crate::diagnostics::DiagnosticsPaths {
                config_dir: self.config_dir.clone(),
                workspace_dir: self.workspace_dir.clone(),
                hub_dir: self.hub_dir.clone(),
            },
            WorkspaceScope::Team => crate::diagnostics::DiagnosticsPaths {
                config_dir: self.hub_dir.clone(),
                workspace_dir: self.hub_dir.clone(),
                hub_dir: self.hub_dir.clone(),
            },
        }
    }

    /// Place a client-supplied path in the logical tree. In the agent scope
    /// `team/...` is the team directory and anything else is relative to the
    /// workspace; in the team scope every path is relative to the team
    /// directory.
    fn locate(&self, relative: &str) -> workspace::Located {
        if self.scope == WorkspaceScope::Team
            && let Some(team) = &self.team
        {
            return workspace::Located {
                base: team.team_root().to_path_buf(),
                rel: relative.to_string(),
                label: relative.to_string(),
                team: Some(team.clone()),
            };
        }
        if let Some(team) = &self.team
            && let Some(rest) = crate::workspace::team_files::team_relative_path(relative)
        {
            let rel = rest
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            return workspace::Located {
                base: team.team_root().to_path_buf(),
                rel,
                label: relative.to_string(),
                team: Some(team.clone()),
            };
        }
        workspace::Located {
            base: self.workspace_dir.clone(),
            rel: relative.to_string(),
            label: relative.to_string(),
            team: None,
        }
    }

    /// The current hub config, reloaded fresh from `hub_dir` (these are
    /// validation/diagnostic paths, not hot paths, so a fresh load is
    /// simpler than threading the running gateway's cached `HubConfig`
    /// through the config API).
    ///
    /// # Errors
    /// Returns `FatalError::Config` if the hub config can't currently be
    /// loaded (e.g. an invalid `hub/config.toml`) — diagnosing the agent's
    /// own config needs the hub's resolved values (timezone, gateway, ...)
    /// to run the same resolution loading does.
    fn hub_config(&self) -> Result<crate::config::HubConfig, crate::util::FatalError> {
        crate::config::HubConfig::load_at(&self.hub_dir)
    }

    /// Checkpoint the agent's own config repository (`config.toml` and
    /// `providers.toml` in the agent's `config/` directory) before a write
    /// to either. Never fails or blocks the write.
    pub(super) async fn checkpoint_agent_config_before_write(&self, summary: impl Into<String>) {
        let _checkpoint_id = self.checkpoint_agent_config_id_before_write(summary).await;
    }

    /// [`Self::checkpoint_agent_config_before_write`], returning the id of
    /// the checkpoint that holds the pre-write tree. `None` when that
    /// checkpoint could not be recorded; the write still proceeds.
    #[must_use]
    pub(super) async fn checkpoint_agent_config_id_before_write(
        &self,
        summary: impl Into<String>,
    ) -> Option<String> {
        self.checkpoints
            .checkpoint_config_kind_id_before_write(
                crate::checkpoints::RepoKind::AgentConfig,
                crate::checkpoints::CheckpointContext::system(
                    crate::checkpoints::CheckpointTrigger::PreConfigWrite,
                    summary,
                ),
            )
            .await
    }

    /// Diagnostics for `contents` as this agent's `config.toml`. A hub
    /// config that currently fails to load surfaces as a single error
    /// diagnostic rather than panicking — semantic validation needs the
    /// hub's resolved values (timezone, gateway, ...) the same way loading
    /// does.
    pub(super) fn diagnose_agent_config(
        &self,
        contents: &str,
    ) -> Vec<crate::diagnostics::Diagnostic> {
        match self.hub_config() {
            Ok(hub) => crate::config::Config::diagnose_agent_toml(
                contents,
                &self.workspace_dir,
                &self.agent_name,
                &hub,
            ),
            Err(e) => vec![crate::diagnostics::Diagnostic::error(format!(
                "hub config couldn't be loaded, so this can't be fully validated: {e}"
            ))],
        }
    }

    /// [`Self::diagnose_agent_config`] for `providers.toml`.
    pub(super) fn diagnose_agent_providers(
        &self,
        contents: &str,
    ) -> Vec<crate::diagnostics::Diagnostic> {
        match self.hub_config() {
            Ok(hub) => crate::config::Config::diagnose_agent_providers_toml(
                contents,
                &self.workspace_dir,
                &self.agent_name,
                &hub,
            ),
            Err(e) => vec![crate::diagnostics::Diagnostic::error(format!(
                "hub config couldn't be loaded, so this can't be fully validated: {e}"
            ))],
        }
    }

    /// Validate `contents` as this agent's `config.toml`, without saving it.
    ///
    /// # Errors
    /// Returns a human-readable error string if validation fails, including
    /// when the hub config can't currently be loaded.
    pub(super) fn validate_agent_config(&self, contents: &str) -> Result<(), String> {
        let hub = self.hub_config().map_err(|e| {
            format!("hub config couldn't be loaded, so this can't be validated: {e}")
        })?;
        crate::config::Config::validate_agent_toml(
            contents,
            &self.workspace_dir,
            &self.agent_name,
            &hub,
        )
    }

    /// [`Self::validate_agent_config`] for `providers.toml`.
    ///
    /// # Errors
    /// Same as [`Self::validate_agent_config`].
    pub(super) fn validate_agent_providers(&self, contents: &str) -> Result<(), String> {
        let hub = self.hub_config().map_err(|e| {
            format!("hub config couldn't be loaded, so this can't be validated: {e}")
        })?;
        crate::config::Config::validate_agent_providers_toml(
            contents,
            &self.workspace_dir,
            &self.agent_name,
            &hub,
        )
    }

    /// Checkpoint the workspace repository before a destructive workspace
    /// API action (delete, overwrite, move/rename with overwrite) or a raw
    /// write to a workspace-owned strictly-parsed file (`mcp.json`,
    /// `config/a2a.json`). Never fails or blocks the action — see
    /// `crate::checkpoints`.
    pub(super) async fn checkpoint_workspace_before_write(&self, summary: impl Into<String>) {
        let _checkpoint_id = self.checkpoint_workspace_id_before_write(summary).await;
    }

    /// [`Self::checkpoint_workspace_before_write`], returning the id of the
    /// checkpoint that holds the pre-action tree. `None` when that checkpoint
    /// could not be recorded; the action still proceeds.
    #[must_use]
    pub(super) async fn checkpoint_workspace_id_before_write(
        &self,
        summary: impl Into<String>,
    ) -> Option<String> {
        self.checkpoints
            .checkpoint_workspace_id_before_action(crate::checkpoints::CheckpointContext::system(
                crate::checkpoints::CheckpointTrigger::PreAction,
                summary,
            ))
            .await
    }

    /// [`Self::checkpoint_workspace_id_before_write`] for the shared team
    /// repository: the checkpoint for a destructive action on a `team/...`
    /// path, which the workspace repository does not contain. `None` when it
    /// could not be recorded; the action still proceeds.
    #[must_use]
    pub(super) async fn checkpoint_team_id_before_write(
        &self,
        summary: impl Into<String>,
    ) -> Option<String> {
        self.checkpoints
            .checkpoint_team_id_before_action(crate::checkpoints::CheckpointContext::system(
                crate::checkpoints::CheckpointTrigger::PreAction,
                summary,
            ))
            .await
    }
}

/// The routes that repair an agent, which work whether or not it is running:
/// its config, providers, MCP, and workspace-file routes, and its workspace
/// and agent-config checkpoints.
///
/// With `reload_tx` set in the state, writes signal the live agent to
/// reload; without it they only touch disk.
pub fn agent_repair_api_router(state: ConfigApiState) -> axum::Router {
    let checkpoints = checkpoints::checkpoints_api_router(
        checkpoints::CheckpointApiState {
            checkpoints: Arc::clone(&state.checkpoints),
            repos: checkpoints::AGENT_REPOS,
        },
        "/api/checkpoints",
    );
    axum::Router::new()
        .route("/api/config/raw", get(config::api_config_raw_get))
        .route("/api/config/raw", put(config::api_config_raw_put))
        .route("/api/config/patch", patch(config::api_config_patch))
        .route("/api/config/validate", post(config::api_config_validate))
        .route("/api/providers/raw", get(providers::api_providers_raw_get))
        .route("/api/providers/raw", put(providers::api_providers_raw_put))
        .route(
            "/api/providers/patch",
            patch(providers::api_providers_patch),
        )
        .route(
            "/api/providers/validate",
            post(providers::api_providers_validate),
        )
        .route(
            "/api/providers/models",
            post(providers::api_provider_models),
        )
        .route("/api/mcp/raw", get(config::api_mcp_raw_get))
        .route("/api/mcp/raw", put(config::api_mcp_raw_put))
        .route("/api/mcp/patch", patch(config::api_mcp_patch))
        .merge(workspace_api_router("/api"))
        .with_state(state)
        .merge(checkpoints)
}

/// The workspace file routes under `prefix`, addressing whichever namespace
/// the state's [`WorkspaceScope`] selects. Used under an agent and, scoped to
/// the team directory, under `/api/team`.
fn workspace_api_router(prefix: &str) -> axum::Router<ConfigApiState> {
    // Scoped to just this route via `route_layer` (which wraps every route
    // already registered on *this* router value): axum's default 2 MiB body
    // limit stays in place for every other endpoint, while text writes get
    // the 8 MiB the workspace file API promises.
    let workspace_file_router = axum::Router::new()
        .route(
            "/workspace/file",
            get(workspace::api_workspace_file_read)
                .put(workspace::api_workspace_file_write)
                .delete(workspace::api_workspace_delete),
        )
        .route(
            "/workspace/raw",
            get(workspace::api_workspace_raw_read).put(workspace::api_workspace_raw_write),
        )
        .route_layer(axum::extract::DefaultBodyLimit::max(
            workspace::TEXT_FILE_LIMIT_BYTES,
        ));

    let routes = axum::Router::new()
        .route("/workspace/files", get(workspace::api_workspace_files))
        .route("/workspace/dir", post(workspace::api_workspace_mkdir))
        .route("/workspace/move", post(workspace::api_workspace_move))
        .route(
            "/workspace/validate",
            post(workspace::api_workspace_validate),
        )
        .merge(workspace_file_router)
        .route("/workspace/tree", get(workspace_bulk::api_workspace_tree))
        .route("/workspace/read", post(workspace_bulk::api_workspace_read));
    axum::Router::new().nest(prefix, routes)
}

/// The team's workspace file API: the workspace routes with every path
/// relative to `team/`, served under `/api/team`.
pub(crate) fn team_workspace_api_router(state: ConfigApiState) -> axum::Router {
    workspace_api_router("/api/team").with_state(state)
}

/// The running agent's `status` route: the one per-agent data route that
/// describes the live process, so it needs a running agent.
pub(crate) fn agent_status_api_router(state: ConfigApiState) -> axum::Router {
    axum::Router::new()
        .route("/api/status", get(config::api_status))
        .with_state(state)
}

/// The hub-level config, secret, and key routes, plus onboarding. These exist
/// once per process and need no running agent.
pub(crate) fn hub_api_router(state: HubApiState) -> axum::Router {
    axum::Router::new()
        .route("/api/hub/config/raw", get(config::api_hub_config_raw_get))
        .route("/api/hub/config/raw", put(config::api_hub_config_raw_put))
        .route("/api/hub/config/patch", patch(config::api_hub_config_patch))
        .route(
            "/api/hub/config/validate",
            post(config::api_hub_config_validate),
        )
        .route(
            "/api/hub/config/complete-setup",
            post(config::api_complete_setup),
        )
        .route(
            "/api/hub/providers/models",
            post(providers::api_provider_models),
        )
        .route("/api/hub/system/timezone", get(config::api_system_timezone))
        .route("/api/hub/mcp-catalog", get(config::api_mcp_catalog))
        .route(
            "/api/hub/agent-keys",
            get(agent_keys::api_agent_keys_list).post(agent_keys::api_agent_keys_set),
        )
        .route(
            "/api/hub/agent-keys/{name}",
            delete(agent_keys::api_agent_keys_delete),
        )
        .route(
            "/api/hub/a2a/keys",
            get(a2a::api_a2a_keys_list).post(a2a::api_a2a_keys_create),
        )
        .route("/api/hub/a2a/keys/{name}", delete(a2a::api_a2a_keys_revoke))
        .route(
            "/api/hub/secrets",
            post(secrets::api_secrets_set).get(secrets::api_secrets_list),
        )
        .route(
            "/api/hub/secrets/{name}",
            delete(secrets::api_secrets_delete),
        )
        .with_state(state)
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "test code uses indexing for clarity"
)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn chat_history_returns_empty_when_no_memory_dir() {
        use axum::Json;
        use axum::extract::{Query, State};

        let state = AgentFilesState::for_test(
            PathBuf::from("/tmp/residuum-test-nonexistent/workspace"),
            None,
        );
        let Json(segment) = chat::api_chat_history(
            State(state),
            Query(chat::ChatHistoryQuery { episode: None }),
        )
        .await
        .unwrap();
        match segment {
            chat::ChatHistorySegment::Recent {
                messages,
                next_cursor,
            } => {
                assert!(messages.is_empty(), "setup mode should have no messages");
                assert!(next_cursor.is_none(), "setup mode should have no cursor");
            }
            chat::ChatHistorySegment::Episode { .. } => {
                panic!("expected Recent segment in setup mode");
            }
        }
    }

    #[tokio::test]
    async fn api_usage_returns_zero_default_when_no_memory_dir() {
        use axum::Json;
        use axum::extract::State;

        let state = AgentFilesState::for_test(
            PathBuf::from("/tmp/residuum-test-nonexistent/workspace"),
            None,
        );
        let Json(totals) = chat::api_usage(State(state)).await;
        assert_eq!(totals, crate::agent::usage::SessionUsageTotals::default());
    }

    #[tokio::test]
    async fn api_usage_reads_the_persisted_totals_file() {
        use axum::Json;
        use axum::extract::State;

        let dir = tempfile::tempdir().unwrap();
        let memory_dir = dir.path().join("memory");
        tokio::fs::create_dir_all(&memory_dir).await.unwrap();
        let mut totals = crate::agent::usage::SessionUsageTotals::default();
        totals.accumulate(Some(crate::inference::Usage {
            input_tokens: 300,
            output_tokens: 60,
            cache_creation_tokens: None,
            cache_read_tokens: None,
        }));
        crate::agent::usage::save_session_usage_totals(
            &memory_dir.join("usage_totals.json"),
            &totals,
        )
        .await;

        let state = AgentFilesState::for_test(dir.path().to_path_buf(), Some(memory_dir));
        let Json(loaded) = chat::api_usage(State(state)).await;
        assert_eq!(loaded, totals);
    }

    #[tokio::test]
    async fn chat_history_returns_empty_when_file_missing() {
        use axum::Json;
        use axum::extract::{Query, State};

        let state = AgentFilesState::for_test(
            PathBuf::from("/tmp/residuum-test-nonexistent/workspace"),
            Some(PathBuf::from("/tmp/residuum-test-nonexistent-memory")),
        );
        let Json(segment) = chat::api_chat_history(
            State(state),
            Query(chat::ChatHistoryQuery { episode: None }),
        )
        .await
        .unwrap();
        match segment {
            chat::ChatHistorySegment::Recent {
                messages,
                next_cursor,
            } => {
                assert!(messages.is_empty(), "missing file should have no messages");
                assert!(next_cursor.is_none(), "no episodes yet");
            }
            chat::ChatHistorySegment::Episode { .. } => {
                panic!("expected Recent segment when recent_messages.json is missing");
            }
        }
    }

    #[tokio::test]
    async fn chat_history_recent_exposes_latest_episode_cursor() {
        use crate::memory::episode_store::{episode_jsonl_path, write_episode_transcript};
        use crate::memory::types::Episode;
        use axum::Json;
        use axum::extract::{Query, State};

        let tmp = tempfile::tempdir().unwrap();
        let memory_dir = tmp.path().join("memory");
        let episodes_dir = memory_dir.join("episodes");
        tokio::fs::create_dir_all(&episodes_dir).await.unwrap();

        // Write two episodes on disk so the cursor should point at ep-002.
        for (id, date) in [
            (
                "ep-001",
                chrono::NaiveDate::from_ymd_opt(2026, 2, 19).unwrap(),
            ),
            (
                "ep-002",
                chrono::NaiveDate::from_ymd_opt(2026, 2, 20).unwrap(),
            ),
        ] {
            let episode = Episode {
                id: id.to_string(),
                date,
                observations: vec![],
            };
            write_episode_transcript(
                &episodes_dir,
                &episode,
                &[crate::inference::Message::user("hi")],
            )
            .await
            .unwrap();
            // Sanity check the file lands where we expect.
            assert!(episode_jsonl_path(&episodes_dir, &episode).exists());
        }

        let state = AgentFilesState::for_test(tmp.path().to_path_buf(), Some(memory_dir));
        let Json(segment) = chat::api_chat_history(
            State(state),
            Query(chat::ChatHistoryQuery { episode: None }),
        )
        .await
        .unwrap();

        match segment {
            chat::ChatHistorySegment::Recent { next_cursor, .. } => {
                assert_eq!(next_cursor.as_deref(), Some("ep-002"));
            }
            chat::ChatHistorySegment::Episode { .. } => panic!("expected Recent"),
        }
    }

    #[tokio::test]
    async fn chat_history_fetches_specific_episode_with_prev_cursor() {
        use crate::memory::episode_store::write_episode_transcript;
        use crate::memory::types::Episode;
        use axum::Json;
        use axum::extract::{Query, State};

        let tmp = tempfile::tempdir().unwrap();
        let memory_dir = tmp.path().join("memory");
        let episodes_dir = memory_dir.join("episodes");
        tokio::fs::create_dir_all(&episodes_dir).await.unwrap();

        for (id, date, body) in [
            (
                "ep-001",
                chrono::NaiveDate::from_ymd_opt(2026, 2, 19).unwrap(),
                "oldest",
            ),
            (
                "ep-002",
                chrono::NaiveDate::from_ymd_opt(2026, 2, 20).unwrap(),
                "middle",
            ),
            (
                "ep-003",
                chrono::NaiveDate::from_ymd_opt(2026, 2, 21).unwrap(),
                "newest",
            ),
        ] {
            let episode = Episode {
                id: id.to_string(),
                date,
                observations: vec![],
            };
            write_episode_transcript(
                &episodes_dir,
                &episode,
                &[crate::inference::Message::user(body)],
            )
            .await
            .unwrap();
        }

        let state = AgentFilesState::for_test(tmp.path().to_path_buf(), Some(memory_dir));

        let Json(segment) = chat::api_chat_history(
            State(state),
            Query(chat::ChatHistoryQuery {
                episode: Some("ep-002".to_string()),
            }),
        )
        .await
        .unwrap();

        match segment {
            chat::ChatHistorySegment::Episode {
                episode_id,
                messages,
                next_cursor,
                ..
            } => {
                assert_eq!(episode_id, "ep-002");
                assert_eq!(messages.len(), 1);
                assert_eq!(messages[0].message.content, "middle");
                assert_eq!(
                    next_cursor.as_deref(),
                    Some("ep-001"),
                    "cursor should walk backward"
                );
            }
            chat::ChatHistorySegment::Recent { .. } => panic!("expected Episode"),
        }
    }

    #[tokio::test]
    async fn chat_history_missing_episode_returns_404() {
        use axum::extract::{Query, State};
        use axum::http::StatusCode;

        let tmp = tempfile::tempdir().unwrap();
        let memory_dir = tmp.path().join("memory");
        tokio::fs::create_dir_all(&memory_dir).await.unwrap();

        let state = AgentFilesState::for_test(tmp.path().to_path_buf(), Some(memory_dir));

        let err = chat::api_chat_history(
            State(state),
            Query(chat::ChatHistoryQuery {
                episode: Some("ep-999".to_string()),
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn chat_history_propagates_parse_errors_instead_of_silently_returning_empty() {
        // Regression test: a malformed recent_messages.json used to be swallowed
        // at debug! level and surfaced as an empty Recent segment, hiding real
        // file corruption and making the chat feed look empty when it wasn't.
        // The handler must now return a 5xx so the frontend can surface the
        // failure to the user instead of silently dropping the history.
        use axum::extract::{Query, State};
        use axum::http::StatusCode;

        let tmp = tempfile::tempdir().unwrap();
        let memory_dir = tmp.path().join("memory");
        tokio::fs::create_dir_all(&memory_dir).await.unwrap();

        // Timestamp uses ISO seconds-precision, which minute_format rejects.
        // A single bad row here fails the whole file parse.
        let malformed = r#"[{"role":"user","content":"hi","timestamp":"2026-04-12T15:00:30","visibility":"user"}]"#;
        tokio::fs::write(memory_dir.join("recent_messages.json"), malformed)
            .await
            .unwrap();

        let state = AgentFilesState::for_test(tmp.path().to_path_buf(), Some(memory_dir));

        let err = chat::api_chat_history(
            State(state),
            Query(chat::ChatHistoryQuery { episode: None }),
        )
        .await
        .unwrap_err();
        assert_eq!(
            err,
            StatusCode::INTERNAL_SERVER_ERROR,
            "parse failures must not be silently converted to empty history",
        );
    }

    #[tokio::test]
    async fn secrets_set_list_delete_roundtrip() {
        use axum::Json;
        use axum::extract::{Path, State};

        let dir = tempfile::tempdir().unwrap();
        let hub_dir = dir.path().join("hub");
        std::fs::create_dir_all(&hub_dir).unwrap();
        let state = HubApiState::for_test(&hub_dir);

        // Set a secret
        let set_result = secrets::api_secrets_set(
            State(state.clone()),
            Json(secrets::SetSecretRequest {
                name: "test_key".to_string(),
                value: "test_value".to_string(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(set_result.0.reference, "secret:test_key");

        // List secrets
        let list_result = secrets::api_secrets_list(State(state.clone()))
            .await
            .unwrap();
        assert_eq!(list_result.0.names, vec!["test_key"]);

        // Delete the secret
        let delete_result =
            secrets::api_secrets_delete(State(state.clone()), Path("test_key".to_string()))
                .await
                .unwrap();
        assert!(delete_result.0.deleted);

        // Verify it's gone
        let after_delete = secrets::api_secrets_list(State(state)).await.unwrap();
        assert!(after_delete.0.names.is_empty());
    }
}

/// API states whose checkpoint engine watches the same config and workspace
/// directories the handlers write to. `test_engine` deliberately does not, so
/// a test that asserts on the returned checkpoint id needs these.
#[cfg(test)]
pub(super) mod test_support {
    use super::{ConfigApiState, HubApiState, WorkspaceScope};

    fn watching_engine(
        root: &std::path::Path,
    ) -> (
        std::path::PathBuf,
        std::path::PathBuf,
        std::path::PathBuf,
        std::sync::Arc<crate::checkpoints::CheckpointEngine>,
    ) {
        let config_dir = root.join("config");
        let workspace_dir = root.join("workspace");
        let hub_dir = root.join("hub");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::create_dir_all(&workspace_dir).unwrap();
        std::fs::create_dir_all(&hub_dir).unwrap();
        let checkpoints = std::sync::Arc::new(
            crate::checkpoints::CheckpointEngine::new(
                "test-agent",
                workspace_dir.clone(),
                &crate::config::paths::TeamPaths::new(hub_dir.clone().join("team")),
                config_dir.clone(),
                hub_dir.clone(),
                &hub_dir.join("checkpoints"),
                None,
            )
            .unwrap(),
        );
        (hub_dir, config_dir, workspace_dir, checkpoints)
    }

    pub(super) fn watching_state(root: &std::path::Path) -> ConfigApiState {
        let (hub_dir, config_dir, workspace_dir, checkpoints) = watching_engine(root);
        ConfigApiState {
            team: None,
            hub_dir,
            config_dir,
            agent_name: "test-agent".to_string(),
            workspace_dir,
            memory_dir: None,
            reload_tx: None,
            checkpoints,
            scope: WorkspaceScope::Agent,
        }
    }

    pub(super) fn watching_hub_state(root: &std::path::Path) -> HubApiState {
        let (hub_dir, _config_dir, _workspace_dir, checkpoints) = watching_engine(root);
        let mut state = HubApiState::for_test(&hub_dir);
        state.checkpoints = checkpoints;
        state
    }
}
