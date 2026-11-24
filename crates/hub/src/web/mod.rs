mod a2a;
mod agent_files;
mod agent_keys;
mod assets;
mod chat;
pub mod checkpoints;
pub mod cloud;
pub(crate) mod cloud_sign_in;
pub mod config;
mod context;
mod inbox;
pub mod providers;
pub mod secrets;
mod system_one;
pub mod teams_setup;
pub mod tracing_api;
pub mod update;
mod usage;
mod workbench;
pub mod workspace;
pub(crate) mod workspace_bulk;
#[cfg(test)]
mod workspace_team_tests;

pub(crate) use assets::static_assets;
pub use assets::{WebAsset, WebAssetSource};
pub use chat::HistoryMessage;
pub use workbench::{WorkbenchApiState, workbench_api_router};

use std::path::PathBuf;
use std::sync::Arc;

use axum::routing::{delete, get, patch, post, put};
use tokio::sync::watch;

pub use agent_files::{AgentFilesState, CheckpointAccess, agent_files_api_router};
pub(crate) use usage::{UsageState, agent_usage_api_router};

#[derive(Clone)]
pub struct ConfigApiState {
    pub hub_dir: PathBuf,
    pub config_dir: PathBuf,
    pub agent_name: String,
    pub workspace_dir: PathBuf,
    pub memory_dir: Option<PathBuf>,
    pub reload_tx: Option<residuum_bus::control::ReloadSender>,
    pub checkpoints: Arc<residuum_checkpoints::CheckpointEngine>,
    pub team: Option<residuum_workspace::team_files::TeamFiles>,
    pub scope: WorkspaceScope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceScope {
    Agent,
    Team,
}

#[derive(Clone)]
pub(crate) struct HubApiState {
    pub hub_dir: PathBuf,
    pub reload_tx: residuum_bus::control::ReloadSender,
    pub setup_done: Option<Arc<watch::Sender<bool>>>,
    pub secret_lock: Arc<tokio::sync::Mutex<()>>,
    pub checkpoints: Arc<residuum_checkpoints::CheckpointEngine>,
    pub team: residuum_workspace::team_files::TeamWriteCoordinator,
    pub assets: WebAssetSource,
}

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
    #[cfg(test)]
    pub(crate) fn for_test(hub_dir: &std::path::Path, assets: WebAssetSource) -> Self {
        let (reload_tx, _reload_rx) = tokio::sync::mpsc::unbounded_channel();
        Self {
            hub_dir: hub_dir.to_path_buf(),
            reload_tx,
            setup_done: None,
            secret_lock: Arc::new(tokio::sync::Mutex::new(())),
            checkpoints: residuum_checkpoints::testing::test_engine(),
            team: residuum_workspace::team_files::TeamWriteCoordinator::new(
                &residuum_config::paths::TeamPaths::new(hub_dir.join("team-for-test")),
            ),
            assets,
        }
    }

    async fn bootstrap_workspace(
        &self,
        layout: &residuum_config::WorkspaceLayout,
        user_name: Option<&str>,
        timezone: &str,
        label: Option<&str>,
    ) -> Result<(), residuum_core::util::FatalError> {
        residuum_workspace::bootstrap::ensure_workspace_labeled(
            layout,
            &self.team,
            user_name,
            Some(timezone),
            label,
        )
        .await
    }

    async fn checkpoint_config_before_write(&self, summary: impl Into<String>) {
        let _checkpoint_id = self.checkpoint_config_id_before_write(summary).await;
    }

    #[must_use]
    async fn checkpoint_config_id_before_write(
        &self,
        summary: impl Into<String>,
    ) -> Option<String> {
        self.checkpoints
            .checkpoint_config_id_before_write(residuum_checkpoints::CheckpointContext::system(
                residuum_checkpoints::CheckpointTrigger::PreConfigWrite,
                summary,
            ))
            .await
    }
}

impl ConfigApiState {
    fn team_mount(&self) -> Option<PathBuf> {
        match self.scope {
            WorkspaceScope::Agent => self.team.as_ref().map(|t| t.team_root().to_path_buf()),
            WorkspaceScope::Team => None,
        }
    }

    fn diagnostics_paths(&self) -> residuum_a2a::diagnostics_dispatch::DiagnosticsPaths {
        match self.scope {
            WorkspaceScope::Agent => residuum_a2a::diagnostics_dispatch::DiagnosticsPaths {
                config_dir: self.config_dir.clone(),
                workspace_dir: self.workspace_dir.clone(),
                hub_dir: self.hub_dir.clone(),
            },
            WorkspaceScope::Team => residuum_a2a::diagnostics_dispatch::DiagnosticsPaths {
                config_dir: self.hub_dir.clone(),
                workspace_dir: self.hub_dir.clone(),
                hub_dir: self.hub_dir.clone(),
            },
        }
    }

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
            && let Some(rest) = residuum_workspace::team_files::team_relative_path(relative)
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

    fn hub_config(&self) -> Result<residuum_config::HubConfig, residuum_core::util::FatalError> {
        residuum_config::HubConfig::load_at(&self.hub_dir)
    }

    async fn checkpoint_agent_config_before_write(&self, summary: impl Into<String>) {
        let _checkpoint_id = self.checkpoint_agent_config_id_before_write(summary).await;
    }

    #[must_use]
    async fn checkpoint_agent_config_id_before_write(
        &self,
        summary: impl Into<String>,
    ) -> Option<String> {
        self.checkpoints
            .checkpoint_config_kind_id_before_write(
                residuum_checkpoints::RepoKind::AgentConfig,
                residuum_checkpoints::CheckpointContext::system(
                    residuum_checkpoints::CheckpointTrigger::PreConfigWrite,
                    summary,
                ),
            )
            .await
    }

    fn diagnose_agent_config(&self, contents: &str) -> Vec<residuum_core::diagnostics::Diagnostic> {
        match self.hub_config() {
            Ok(hub) => residuum_config::Config::diagnose_agent_toml(
                contents,
                &self.workspace_dir,
                &self.agent_name,
                &hub,
            ),
            Err(e) => vec![residuum_core::diagnostics::Diagnostic::error(format!(
                "hub config couldn't be loaded, so this can't be fully validated: {e}"
            ))],
        }
    }

    fn diagnose_agent_providers(
        &self,
        contents: &str,
    ) -> Vec<residuum_core::diagnostics::Diagnostic> {
        match self.hub_config() {
            Ok(hub) => residuum_config::Config::diagnose_agent_providers_toml(
                contents,
                &self.workspace_dir,
                &self.agent_name,
                &hub,
            ),
            Err(e) => vec![residuum_core::diagnostics::Diagnostic::error(format!(
                "hub config couldn't be loaded, so this can't be fully validated: {e}"
            ))],
        }
    }

    fn validate_agent_config(&self, contents: &str) -> Result<(), String> {
        let hub = self.hub_config().map_err(|e| {
            format!("hub config couldn't be loaded, so this can't be validated: {e}")
        })?;
        residuum_config::Config::validate_agent_toml(
            contents,
            &self.workspace_dir,
            &self.agent_name,
            &hub,
        )
    }

    fn validate_agent_providers(&self, contents: &str) -> Result<(), String> {
        let hub = self.hub_config().map_err(|e| {
            format!("hub config couldn't be loaded, so this can't be validated: {e}")
        })?;
        residuum_config::Config::validate_agent_providers_toml(
            contents,
            &self.workspace_dir,
            &self.agent_name,
            &hub,
        )
    }

    async fn checkpoint_workspace_before_write(&self, summary: impl Into<String>) {
        let _checkpoint_id = self.checkpoint_workspace_id_before_write(summary).await;
    }

    #[must_use]
    async fn checkpoint_workspace_id_before_write(
        &self,
        summary: impl Into<String>,
    ) -> Option<String> {
        self.checkpoints
            .checkpoint_workspace_id_before_action(residuum_checkpoints::CheckpointContext::system(
                residuum_checkpoints::CheckpointTrigger::PreAction,
                summary,
            ))
            .await
    }

    #[must_use]
    async fn checkpoint_team_id_before_write(&self, summary: impl Into<String>) -> Option<String> {
        self.checkpoints
            .checkpoint_team_id_before_action(residuum_checkpoints::CheckpointContext::system(
                residuum_checkpoints::CheckpointTrigger::PreAction,
                summary,
            ))
            .await
    }
}

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
        .merge(teams_setup::teams_setup_api_router())
        .with_state(state)
        .merge(checkpoints)
}

fn workspace_api_router(prefix: &str) -> axum::Router<ConfigApiState> {
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

pub(crate) fn team_workspace_api_router(state: ConfigApiState) -> axum::Router {
    workspace_api_router("/api/team").with_state(state)
}

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
        .route(
            "/api/hub/system-one/models",
            post(system_one::api_system_one_models),
        )
        .route(
            "/api/hub/system-one/test",
            post(system_one::api_system_one_test),
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
        assert_eq!(totals, residuum_bus::usage::SessionUsageTotals::default());
    }

    #[tokio::test]
    async fn api_usage_reads_the_persisted_totals_file() {
        use axum::Json;
        use axum::extract::State;

        let dir = tempfile::tempdir().unwrap();
        let memory_dir = dir.path().join("memory");
        tokio::fs::create_dir_all(&memory_dir).await.unwrap();
        let mut totals = residuum_bus::usage::SessionUsageTotals::default();
        totals.accumulate(Some(residuum_inference::Usage {
            uncached_input_tokens: Some(300),
            output_tokens: Some(60),
            ..Default::default()
        }));
        residuum_agent::usage::save_session_usage_totals(
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
        use axum::Json;
        use axum::extract::{Query, State};
        use residuum_memory::episode_store::{episode_jsonl_path, write_episode_transcript};
        use residuum_memory::types::Episode;

        let tmp = tempfile::tempdir().unwrap();
        let memory_dir = tmp.path().join("memory");
        let episodes_dir = memory_dir.join("episodes");
        tokio::fs::create_dir_all(&episodes_dir).await.unwrap();

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
                &[residuum_inference::Message::user("hi")],
            )
            .await
            .unwrap();
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
        use axum::Json;
        use axum::extract::{Query, State};
        use residuum_memory::episode_store::write_episode_transcript;
        use residuum_memory::types::Episode;

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
                &[residuum_inference::Message::user(body)],
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
    async fn chat_history_pages_only_through_the_main_conversation() {
        use axum::Json;
        use axum::extract::{Query, State};
        use residuum_memory::episode_store::write_episode_transcript_tagged;
        use residuum_memory::types::{Episode, SourceTag};

        let tmp = tempfile::tempdir().unwrap();
        let memory_dir = tmp.path().join("memory");
        let episodes_dir = memory_dir.join("episodes");
        let wiki_lint = SourceTag::session("scheduled-wiki-lint-2b42", "run-1", "scheduled");
        for (id, tag) in [
            ("ep-001", SourceTag::main()),
            ("ep-002", wiki_lint.clone()),
            ("ep-003", SourceTag::main()),
            ("ep-004", wiki_lint),
        ] {
            let episode = Episode {
                id: id.to_string(),
                date: chrono::NaiveDate::from_ymd_opt(2026, 10, 6).unwrap(),
                observations: vec![],
            };
            write_episode_transcript_tagged(
                &episodes_dir,
                &episode,
                &[residuum_inference::Message::user(id)],
                &tag,
                None,
            )
            .await
            .unwrap();
        }
        let state = AgentFilesState::for_test(tmp.path().to_path_buf(), Some(memory_dir));
        let history = |episode: Option<&str>| {
            chat::api_chat_history(
                State(state.clone()),
                Query(chat::ChatHistoryQuery {
                    episode: episode.map(str::to_string),
                }),
            )
        };

        let Json(recent) = history(None).await.unwrap();
        let chat::ChatHistorySegment::Recent { next_cursor, .. } = recent else {
            panic!("expected Recent");
        };
        assert_eq!(
            next_cursor.as_deref(),
            Some("ep-003"),
            "the newest episode is a pulse run, not main history"
        );

        let Json(newest) = history(Some("ep-003")).await.unwrap();
        let chat::ChatHistorySegment::Episode {
            next_cursor: older, ..
        } = newest
        else {
            panic!("expected Episode");
        };
        assert_eq!(older.as_deref(), Some("ep-001"));

        assert_eq!(
            history(Some("ep-002")).await.unwrap_err(),
            axum::http::StatusCode::NOT_FOUND,
            "a session run's episode is never served as main chat history"
        );
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
        use axum::extract::{Query, State};
        use axum::http::StatusCode;

        let tmp = tempfile::tempdir().unwrap();
        let memory_dir = tmp.path().join("memory");
        tokio::fs::create_dir_all(&memory_dir).await.unwrap();

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
        let state = HubApiState::for_test(&hub_dir, crate::testing::fixture_assets);

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

        let list_result = secrets::api_secrets_list(State(state.clone()))
            .await
            .unwrap();
        assert_eq!(list_result.0.names, vec!["test_key"]);

        let delete_result =
            secrets::api_secrets_delete(State(state.clone()), Path("test_key".to_string()))
                .await
                .unwrap();
        assert!(delete_result.0.deleted);

        let after_delete = secrets::api_secrets_list(State(state)).await.unwrap();
        let actual = &after_delete.0.names;
        assert!(actual.is_empty(), "expected {actual:?} to be empty");
    }
}
