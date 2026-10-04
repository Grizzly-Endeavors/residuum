//! Services the hub builds once and passes into every agent it hosts.
//!
//! An agent depends on no process-wide mutable state: everything it reads
//! from the hub arrives here, as an explicit handle. Each field is either
//! cheap to clone (an `Arc`, a channel end) or a handle that is already
//! shared by construction.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::{Mutex, Semaphore, mpsc, watch};
use tracing::Instrument as _;

use crate::a2a::SharedA2aKeys;
use crate::agent_keys::SharedAgentKeys;
use crate::bus::BusHandle;
use crate::checkpoints::SharedCheckpointRepos;
use crate::config::paths::TeamPaths;
use crate::config::{HubConfig, HubPaths};
use crate::hub::push::PushService;
use crate::inference::EmbeddingProvider;
use crate::inference::system_one::SystemOneService;
use crate::memory::team_wiki::TeamWikiIndex;
use crate::tracing_service::TracingService;
use crate::tunnel::TunnelStatus;
use crate::update::SharedUpdateStatus;
use crate::util::FatalError;
use crate::workspace::team_files::TeamWriteCoordinator;
use crate::workspace::watch::WatchHealth;

/// The hub-level controls an agent's HTTP routes drive: the update status
/// and the requests to restart or shut down the whole process.
#[derive(Clone)]
pub(crate) struct HubControl {
    /// Latest result of the periodic version check.
    pub update_status: SharedUpdateStatus,
    /// Asks the hub to stop every agent and relaunch the binary.
    pub restart_tx: mpsc::Sender<()>,
    /// Asks the hub to stop every agent and exit.
    pub shutdown_tx: mpsc::Sender<()>,
}

/// The hub's one change feed over the team directory.
///
/// It publishes `WorkspaceEvent`s on `topics::Workspace` of its own bus, with
/// paths under the `team/` prefix. The hub WebSocket, every agent's
/// WebSocket (for clients watching `team/...`), and every agent's artifact
/// reload watcher read it, so a change to a team file is watched once no
/// matter how many agents are running.
///
/// The hub also watches the team workbench itself, from this feed, and
/// publishes a `WorkbenchEvent` on `topics::Workbench` of the same bus for
/// every artifact added, changed or removed. The hub WebSocket sends them as
/// artifact events, which therefore reach clients with no agent running.
pub(crate) struct TeamChangeFeed {
    /// The bus the feed publishes on.
    pub bus: BusHandle,
    /// Whether the feed is running, so a client that starts watching can be
    /// told when live updates are off.
    pub health: watch::Receiver<WatchHealth>,
    task: tokio::task::JoinHandle<()>,
    /// The hub's artifact watcher; `None` when it couldn't subscribe to the
    /// feed, which was logged.
    workbench_task: Option<tokio::task::JoinHandle<()>>,
}

impl TeamChangeFeed {
    /// Start the feed over `team_root`, creating the directory if it is
    /// missing so the watcher has something to watch.
    pub(crate) async fn start(team_root: PathBuf) -> Self {
        if let Err(e) = tokio::fs::create_dir_all(&team_root).await {
            tracing::warn!(error = %e, path = %team_root.display(), "failed to create the team directory; changes to team files may not appear live");
        }
        // The feed serves every agent, so the log lines of its broker and
        // watcher belong to the hub's team feed, not to any one agent.
        let span = tracing::info_span!("team_feed");
        let bus = span.in_scope(crate::bus::spawn_broker);
        let (health_tx, health) = watch::channel(WatchHealth::Starting);
        // Subscribed before the feed starts, so no batch goes unseen.
        let workbench_task = crate::workbench::watcher::spawn_workbench_watcher(
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
            crate::workspace::watch::spawn_change_feed(
                team_root,
                Some(crate::workspace::team_files::TEAM_PREFIX),
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

    /// A feed that watches nothing, for tests.
    #[cfg(test)]
    pub(crate) fn idle() -> Self {
        Self {
            bus: crate::bus::spawn_broker(),
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

/// Shared services handed to every agent's runtime.
#[derive(Clone)]
pub(crate) struct HubServices {
    /// The residuum root (`~/.residuum`): parent of `hub/`, `team/` and
    /// every agent directory.
    pub root: PathBuf,
    /// The hub's own directory (`~/.residuum/hub`).
    pub hub_dir: PathBuf,
    /// Coordinates writes to the team directory across all agents and the
    /// web API. Each agent takes its own view with
    /// [`TeamWriteCoordinator::view_for_agent`].
    pub team: TeamWriteCoordinator,
    /// The team wiki's search index. Opened once; every agent's searcher
    /// holds a clone of this handle.
    pub team_wiki: Arc<TeamWikiIndex>,
    /// Hub-wide budget for concurrently running session turns, sized by the
    /// hub's `[background] max_concurrent` and shared by every agent's
    /// session runtime.
    pub session_budget: Arc<Semaphore>,
    /// Status of the relay tunnel.
    pub tunnel_status_rx: watch::Receiver<TunnelStatus>,
    /// The shared agent-key store (credentials injected into commands and
    /// MCP servers).
    pub agent_keys: SharedAgentKeys,
    /// The shared store of A2A caller keys.
    pub a2a_keys: SharedA2aKeys,
    /// Trace export, bug reports, and the span buffer.
    pub tracing_service: Arc<TracingService>,
    /// The team and hub-config checkpoint repositories every agent's
    /// checkpoint engine records into.
    pub checkpoints: Arc<SharedCheckpointRepos>,
    /// Serializes writes to the secret store, which every agent's settings
    /// routes share.
    pub secret_lock: Arc<Mutex<()>>,
    /// Update and process-control handles.
    pub control: HubControl,
    /// Whether the workbench artifacts listener is running, and on which port.
    pub workbench_serving: crate::workbench::server::WorkbenchServing,
    /// The one change feed over the team directory.
    pub team_feed: Arc<TeamChangeFeed>,
    /// Feeds relay-discovered sibling instances to every registered agent's
    /// A2A client hub. Each agent registers at start and unregisters at stop.
    pub sibling_fanout: Arc<crate::a2a::SiblingFanout>,
    /// Carries `agent:` messages between the running agents. Each agent
    /// registers its messenger at start and unregisters at stop.
    pub team_router: Arc<super::team::TeamRouter>,
    /// The agent host, for agent tools that create or delete teammates. The
    /// host binds itself when it is built.
    pub directory: super::directory::DirectoryHandle,
    /// Web Push: registered devices and delivery. Triggers send through it.
    pub push: Arc<PushService>,
    /// The System 1 (decision model) client every agent shares, with its
    /// health. Rebuilt in place when `[system_one]` changes.
    pub system_one: Arc<SystemOneService>,
}

impl HubServices {
    /// Build the shared services for the hub rooted at `root`.
    ///
    /// `team_embedding` is the provider that embeds team wiki pages; `None`
    /// leaves wiki search text-only.
    ///
    /// # Errors
    /// Returns `FatalError` if the team wiki index or the shared checkpoint
    /// repositories cannot be opened.
    pub async fn open(
        root: &Path,
        hub: &HubConfig,
        tunnel_status_rx: watch::Receiver<TunnelStatus>,
        control: HubControl,
        workbench_serving: crate::workbench::server::WorkbenchServing,
        team_embedding: Option<Arc<dyn EmbeddingProvider>>,
    ) -> Result<Self, FatalError> {
        let hub_dir = hub.config_dir.clone();
        let team_paths = TeamPaths::new(crate::config::paths::team_dir(root));
        let team = TeamWriteCoordinator::new(&team_paths);
        let team_wiki = TeamWikiIndex::open(&team_paths, team_embedding)
            .await
            .map_err(|e| FatalError::Gateway(format!("failed to open the team wiki index: {e}")))?;
        let agent_keys = crate::agent_keys::AgentKeys::new_shared(&hub_dir);
        let tracing_service = Arc::new(
            TracingService::new(hub.tracing.clone(), crate::util::telemetry::span_buffer())
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
        let directory = super::directory::DirectoryHandle::unbound();
        let sibling_fanout = crate::a2a::SiblingFanout::new_shared();
        crate::a2a::spawn_sibling_discovery(Arc::clone(&sibling_fanout), tunnel_status_rx.clone());
        Ok(Self {
            root: root.to_path_buf(),
            a2a_keys: crate::a2a::A2aKeys::new_shared(&hub_dir),
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
            team_router: super::team::TeamRouter::new_shared(directory.clone()),
            directory,
        })
    }

    /// Shared services over a throwaway hub rooted at `root`, with a
    /// disconnected tunnel and a text-only team wiki, for tests.
    #[cfg(test)]
    pub(crate) async fn for_tests(root: &Path, hub: &HubConfig) -> Self {
        let (_status_tx, tunnel_status_rx) = watch::channel(TunnelStatus::Disconnected);
        let (restart_tx, _restart_rx) = mpsc::channel(1);
        let (shutdown_tx, _shutdown_rx) = mpsc::channel(1);
        Self::open(
            root,
            hub,
            tunnel_status_rx,
            HubControl {
                update_status: SharedUpdateStatus::default(),
                restart_tx,
                shutdown_tx,
            },
            crate::workbench::server::WorkbenchServing::Unavailable {
                reason: "not started in tests".to_string(),
            },
            None,
        )
        .await
        .unwrap()
    }
}
