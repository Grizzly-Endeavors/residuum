use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{mpsc, watch};

use crate::agent_watch::AgentChangeFeed;
use crate::overview::TeamOverview;
use crate::push::PushService;
use crate::team_events::TeamEventLog;
use crate::update::SharedUpdateStatus;
use crate::uploads::UploadStore;
use crate::web::WebAssetSource;
use residuum_bus::BusHandle;
use residuum_bus::control::ReloadSender;
use residuum_checkpoints::CheckpointEngine;
use residuum_core::build_info::BuildInfo;
use residuum_inference::system_one::SystemOneService;
use residuum_net::tunnel::TunnelStatus;
use residuum_net::workbench::WorkbenchServing;
use residuum_tracing_service::{ClientContext, Subagent, TracingService};
use residuum_workspace::team_files::TeamWriteCoordinator;
use residuum_workspace::watch::WatchHealth;

#[derive(Clone)]
pub struct HubHttpState {
    pub hub_dir: PathBuf,
    pub reload_tx: ReloadSender,
    pub setup_done: Option<Arc<watch::Sender<bool>>>,
    pub secret_lock: Arc<tokio::sync::Mutex<()>>,
    pub checkpoints: Arc<CheckpointEngine>,
    pub tunnel_status_rx: watch::Receiver<TunnelStatus>,
    pub build: BuildInfo,
    pub update_status: SharedUpdateStatus,
    pub restart_tx: mpsc::Sender<()>,
    pub shutdown_tx: mpsc::Sender<()>,
    pub tracing_service: Arc<TracingService>,
    pub client_context: Arc<ClientContext>,
    pub active_subagents: Arc<dyn Fn() -> Vec<Subagent> + Send + Sync>,
    pub workbench_serving: WorkbenchServing,
    pub team: TeamWriteCoordinator,
    pub team_bus: BusHandle,
    pub team_watch_health: watch::Receiver<WatchHealth>,
    pub started_at: Instant,
    pub push: Arc<PushService>,
    pub team_events: Arc<TeamEventLog>,
    pub overview: Arc<TeamOverview>,
    pub agent_changes: Arc<AgentChangeFeed>,
    pub system_one: Arc<SystemOneService>,
    /// Which browsers may reach this install through Residuum Cloud.
    pub pairing: residuum_net::pairing::DevicePairing,
    pub remote_access: residuum_net::remote_access::slot::RemoteAccessSlot,
    pub uploads: UploadStore,
    pub assets: WebAssetSource,
}
