//! The web API's modules, the embedded web app, and the running agent's
//! status router.
//!
//! Embeds the `web/dist/` directory into the binary via `rust-embed`.
//! In debug builds, files are served from disk (hot-reload); in release
//! builds, they are compiled into the binary.
//!
//! The routes the hub serves (hub-level, an agent's repair routes and its
//! file routes) live in `hub_api`, `config`, `hub_a2a`, `hub_inbox` and the
//! other hub-served modules; `status`, `a2a`, `inbox`, `sessions`,
//! `scheduled`, `memory` and `model` hold the running agent's routes.

use axum::routing::get;

pub(crate) mod a2a;
mod agent_files;
mod agent_keys;
pub(crate) mod artifact_identity;
mod assets;
pub(super) mod chat;
pub mod checkpoints;
pub mod cloud;
pub(crate) mod cloud_sign_in;
pub mod config;
mod hub_a2a;
mod hub_api;
mod hub_inbox;
pub mod inbox;
pub(crate) mod memory;
pub(crate) mod model;
pub mod providers;
pub(crate) mod scheduled;
pub mod secrets;
pub(crate) mod sessions;
mod status;
mod system_one;
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
pub(crate) use status::AgentStatusState;

pub use agent_files::{AgentFilesState, CheckpointAccess, agent_files_api_router};
#[cfg(test)]
use hub_api::test_support;
pub use hub_api::{ConfigApiState, WorkspaceScope, agent_repair_api_router};
pub(crate) use hub_api::{HubApiState, HubDir, hub_api_router, team_workspace_api_router};

/// The running agent's `status` route: the one per-agent data route that
/// describes the live process, so it needs a running agent.
pub(crate) fn agent_status_api_router(state: AgentStatusState) -> axum::Router {
    axum::Router::new()
        .route("/api/status", get(status::api_status))
        .with_state(state)
}
