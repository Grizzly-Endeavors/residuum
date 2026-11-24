use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::{Mutex, Semaphore, mpsc, watch};
use tracing::Instrument as _;

use crate::push::PushService;
use crate::update::SharedUpdateStatus;
use crate::uploads::UploadStore;
use residuum_a2a::SharedA2aKeys;
use residuum_bus::BusHandle;
use residuum_checkpoints::SharedCheckpointRepos;
use residuum_config::agent_keys::SharedAgentKeys;
use residuum_config::paths::TeamPaths;
use residuum_config::{HubConfig, HubPaths};
use residuum_core::util::FatalError;
use residuum_inference::EmbeddingProvider;
use residuum_inference::system_one::SystemOneService;
use residuum_memory::team_wiki::TeamWikiIndex;
use residuum_net::tunnel::TunnelStatus;
use residuum_tracing_service::TracingService;
use residuum_workspace::team_files::TeamWriteCoordinator;
use residuum_workspace::watch::WatchHealth;

#[derive(Clone)]
pub struct HubControl {
    pub build: residuum_core::build_info::BuildInfo,
    pub update_status: SharedUpdateStatus,
    pub restart_tx: mpsc::Sender<()>,
    pub shutdown_tx: mpsc::Sender<()>,
}

impl HubControl {
    pub(crate) fn new(
        build: residuum_core::build_info::BuildInfo,
        restart_tx: mpsc::Sender<()>,
        shutdown_tx: mpsc::Sender<()>,
    ) -> Self {
        Self {
            build,
            update_status: crate::update::shared_update_status(build.version),
            restart_tx,
            shutdown_tx,
        }
    }
}

pub struct TeamChangeFeed {
    pub bus: BusHandle,
    pub health: watch::Receiver<WatchHealth>,
    task: tokio::task::JoinHandle<()>,
    workbench_task: Option<tokio::task::JoinHandle<()>>,
}

impl TeamChangeFeed {
    pub(crate) async fn start(team_root: PathBuf) -> Self {
        if let Err(e) = tokio::fs::create_dir_all(&team_root).await {
            tracing::warn!(error = %e, path = %team_root.display(), "failed to create the team directory; changes to team files may not appear live");
        }
        let span = tracing::info_span!("team_feed");
        let bus = span.in_scope(residuum_bus::spawn_broker);
        let (health_tx, health) = watch::channel(WatchHealth::Starting);
        let workbench_task = crate::workbench_watcher::spawn_workbench_watcher(
            TeamPaths::new(team_root.clone()).workbench_dir(),
            &bus,
            bus.publisher(),
        )
        .instrument(span.clone())
        .await
        .inspect_err(|e| {
            tracing::warn!(error = %e, "failed to subscribe the hub's artifact watcher to the team change feed; artifact lists won't update on their own");
        })
        .ok();
        let task = span.in_scope(|| {
            residuum_workspace::watch::spawn_change_feed(
                team_root,
                Some(residuum_workspace::team_files::TEAM_PREFIX),
                bus.publisher(),
                health_tx,
            )
        });
        Self {
            bus,
            health,
            task,
            workbench_task,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn idle() -> Self {
        Self {
            bus: residuum_bus::spawn_broker(),
            health: watch::channel(WatchHealth::Native).1,
            task: tokio::spawn(std::future::ready(())),
            workbench_task: None,
        }
    }
}

impl Drop for TeamChangeFeed {
    fn drop(&mut self) {
        self.task.abort();
        if let Some(task) = &self.workbench_task {
            task.abort();
        }
    }
}

#[derive(Clone)]
pub struct HubServices {
    pub root: PathBuf,
    pub hub_dir: PathBuf,
    pub team: TeamWriteCoordinator,
    pub team_wiki: Arc<TeamWikiIndex>,
    pub session_budget: Arc<Semaphore>,
    pub tunnel_status_rx: watch::Receiver<TunnelStatus>,
    pub agent_keys: SharedAgentKeys,
    pub a2a_keys: SharedA2aKeys,
    pub tracing_service: Arc<TracingService>,
    pub checkpoints: Arc<SharedCheckpointRepos>,
    pub secret_lock: Arc<Mutex<()>>,
    pub control: HubControl,
    pub workbench_serving: residuum_net::workbench::WorkbenchServing,
    pub team_feed: Arc<TeamChangeFeed>,
    pub sibling_fanout: Arc<residuum_a2a::SiblingFanout>,
    pub team_router: Arc<residuum_background::team::TeamRouter>,
    pub directory: residuum_background::team::DirectoryHandle,
    pub mailboxes: residuum_workspace::inbox::mail::MailboxDirectoryHandle,
    pub push: Arc<PushService>,
    pub system_one: Arc<SystemOneService>,
    /// Which browsers may reach this install through Residuum Cloud. The
    /// main router and the workbench listener share it.
    pub pairing: residuum_net::pairing::DevicePairing,
    pub remote_access: residuum_net::remote_access::slot::RemoteAccessSlot,
    pub uploads: UploadStore,
}

impl HubServices {
    pub async fn open(
        root: &Path,
        hub: &HubConfig,
        tunnel_status_rx: watch::Receiver<TunnelStatus>,
        control: HubControl,
        workbench_serving: residuum_net::workbench::WorkbenchServing,
        pairing: residuum_net::pairing::DevicePairing,
        team_embedding: Option<Arc<dyn EmbeddingProvider>>,
    ) -> Result<Self, FatalError> {
        let hub_dir = hub.config_dir.clone();
        let team_paths = TeamPaths::new(residuum_config::paths::team_dir(root));
        let team = TeamWriteCoordinator::new(&team_paths);
        let team_wiki = TeamWikiIndex::open(&team_paths, team_embedding)
            .await
            .map_err(|e| FatalError::Gateway(format!("failed to open the team wiki index: {e}")))?;
        let agent_keys = residuum_config::agent_keys::AgentKeys::new_shared(&hub_dir);
        let tracing_service = Arc::new(
            TracingService::new(
                hub.tracing.clone(),
                residuum_core::util::telemetry::span_buffer(),
            )
            .with_agent_keys(Arc::clone(&agent_keys)),
        );
        let checkpoints_dir = HubPaths::new(&hub_dir).checkpoints_dir();
        let checkpoints = SharedCheckpointRepos::open(&hub_dir, &team_paths, &checkpoints_dir)
            .map_err(|e| {
                FatalError::Config(format!(
                    "failed to open checkpoint repositories at {}: {e}",
                    checkpoints_dir.display()
                ))
            })?;
        let team_feed = Arc::new(TeamChangeFeed::start(team_paths.root().to_path_buf()).await);
        let directory = residuum_background::team::DirectoryHandle::unbound();
        let remote_access = residuum_net::remote_access::slot::RemoteAccessSlot::new(&hub_dir);
        let uploads = UploadStore::open(&hub_dir).map_err(|e| {
            FatalError::Config(format!(
                "failed to open the upload store under {}: {e}",
                hub_dir.display()
            ))
        })?;
        if let Err(err) = uploads.prune(chrono::Utc::now()).await {
            tracing::warn!(error = %err, "failed to prune expired staged uploads at startup");
        }
        let sibling_fanout = residuum_a2a::SiblingFanout::new_shared();
        residuum_a2a::spawn_sibling_discovery(
            Arc::clone(&sibling_fanout),
            remote_access.discovery_receiver(),
        );
        Ok(Self {
            root: root.to_path_buf(),
            a2a_keys: residuum_a2a::A2aKeys::new_shared(&hub_dir),
            push: PushService::new(&hub_dir, hub.push.contact.as_deref()),
            system_one: SystemOneService::new(hub.system_one.as_ref()),
            hub_dir,
            team,
            team_wiki,
            session_budget: Arc::new(Semaphore::new(hub.background.max_concurrent)),
            tunnel_status_rx,
            agent_keys,
            tracing_service,
            checkpoints,
            secret_lock: Arc::new(Mutex::new(())),
            control,
            workbench_serving,
            team_feed,
            sibling_fanout,
            team_router: residuum_background::team::TeamRouter::new_shared(directory.clone()),
            directory,
            mailboxes: residuum_workspace::inbox::mail::MailboxDirectoryHandle::unbound(),
            pairing,
            remote_access,
            uploads,
        })
    }

    #[cfg(any(test, feature = "test-support"))]
    #[expect(
        clippy::unwrap_used,
        reason = "test-support helper: services that cannot be opened fail the test that asked for them"
    )]
    pub async fn for_tests(root: &Path, hub: &HubConfig) -> Self {
        let (_status_tx, tunnel_status_rx) = watch::channel(TunnelStatus::Disconnected);
        let (restart_tx, _restart_rx) = mpsc::channel(1);
        let (shutdown_tx, _shutdown_rx) = mpsc::channel(1);
        Self::open(
            root,
            hub,
            tunnel_status_rx,
            HubControl::new(
                residuum_core::testing::HARNESS_BUILD,
                restart_tx,
                shutdown_tx,
            ),
            residuum_net::workbench::WorkbenchServing::Unavailable {
                reason: "not started in tests".to_string(),
            },
            residuum_net::pairing::DevicePairing::open(&hub.config_dir),
            None,
        )
        .await
        .unwrap()
    }
}
