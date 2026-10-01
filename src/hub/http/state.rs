//! The hub-level handles the hub HTTP routes need.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{mpsc, watch};

use crate::bus::BusHandle;
use crate::checkpoints::CheckpointEngine;
use crate::gateway::types::ReloadSender;
use crate::hub::push::PushService;
use crate::tracing_service::{ClientContext, Subagent, TracingService};
use crate::tunnel::TunnelStatus;
use crate::update::SharedUpdateStatus;
use crate::workbench::server::WorkbenchServing;
use crate::workspace::team_files::TeamWriteCoordinator;
use crate::workspace::watch::WatchHealth;

/// Everything the hub routes read or signal that is not a hosted agent: the
/// hub directory, the tunnel and update state, the tracing service, the team
/// directory, and the channels back to the process (reload, restart,
/// shutdown).
///
/// The agent host builds one when the hub starts and passes it to
/// [`super::hub_router`] together with its `AgentDirectory`. Every handle is
/// process-wide: none of them belongs to a single agent.
#[derive(Clone)]
pub struct HubHttpState {
    /// The hub directory (`~/.residuum/hub`): `config.toml`, the encrypted
    /// secret, agent-key and A2A-key stores, and the checkpoint repositories.
    pub hub_dir: PathBuf,
    /// Where the hub is told to reload after a hub-owned file changes (the
    /// hub config, a key store, the cloud settings). It also carries
    /// `ReloadSignal::Workspace` when an edit through the team file API
    /// changes a team identity file (`AGENTS.md`, `USER.md`): the host
    /// reloads every running agent's workspace on it.
    pub reload_tx: ReloadSender,
    /// Set while the hub has no agents. Onboarding
    /// (`POST /api/hub/config/complete-setup`) signals it once the first
    /// agent's directory is on disk, and the host then discovers and starts
    /// that agent. `None` once agents exist, when setup can no longer run.
    pub setup_done: Option<Arc<watch::Sender<bool>>>,
    /// Serializes secret store writes across the hub routes and the cloud
    /// callback, so concurrent writes can't lose an update.
    pub secret_lock: Arc<tokio::sync::Mutex<()>>,
    /// The checkpoint repositories. The hub routes serve only the `hub` and
    /// `team` repositories from it.
    pub checkpoints: Arc<CheckpointEngine>,
    /// The relay tunnel's status.
    pub tunnel_status_rx: watch::Receiver<TunnelStatus>,
    /// The update checker's shared status.
    pub update_status: SharedUpdateStatus,
    /// Asks the process to restart itself (after an update, or on request).
    pub restart_tx: mpsc::Sender<()>,
    /// Asks the process to shut down, stopping every agent.
    pub shutdown_tx: mpsc::Sender<()>,
    /// The tracing service the hub's tracing routes drive.
    pub tracing_service: Arc<TracingService>,
    /// Static runtime context (version, OS, ...) attached to bug reports.
    pub client_context: Arc<ClientContext>,
    /// The live subagent sessions across every running agent, read on each
    /// bug report.
    pub active_subagents: Arc<dyn Fn() -> Vec<Subagent> + Send + Sync>,
    /// Whether the artifacts listener is running, and where.
    pub workbench_serving: WorkbenchServing,
    /// Coordinates writes under the team directory. The team file API writes
    /// through a view of it attributed to the user.
    pub team: TeamWriteCoordinator,
    /// A bus carrying the team change feed: `WorkspaceEvent`s on
    /// `topics::Workspace` whose paths carry the `team/` prefix. The hub
    /// WebSocket forwards them to clients that ask to watch team paths.
    pub team_bus: BusHandle,
    /// Whether the team change feed is running, so a client that starts
    /// watching can be told when live updates are off.
    pub team_watch_health: watch::Receiver<WatchHealth>,
    /// When the hub started, for `uptime_secs` in `GET /api/hub/status`.
    pub started_at: Instant,
    /// Web Push: the signing key, the registered devices, and delivery.
    pub push: Arc<PushService>,
    /// A random id this hub process generated at startup. The hub WebSocket
    /// sends it first on every connection, so a client can tell a restarted
    /// hub from a dropped connection to the same one.
    pub boot_id: String,
}
