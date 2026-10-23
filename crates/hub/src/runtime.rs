//! The hub process: everything that is one-per-process rather than
//! one-per-agent.
//!
//! [`run_hub`] starts the HTTP server, the relay tunnel, the A2A listener,
//! the workbench server, tracing, and the updater, builds the services every
//! agent shares (see `HubServices`), and hands the agents to the
//! [`AgentHost`]. It runs until the process is told to stop, then stops every
//! agent gracefully.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::Duration;

use residuum_bus::control::{ReloadReceiver, ReloadSignal};
use residuum_config::{Config, HubConfig, LogLevel};
use residuum_core::build_info::BuildInfo;
use residuum_core::util::FatalError;
use residuum_inference::EmbeddingProvider;
use residuum_net::tunnel::TunnelStatus;
use residuum_net::workbench::{HubApi, SdkContext};

use super::config_watcher;
use super::directory::AgentDirectory;
use super::host::AgentHost;
use super::http::{HubHttpState, hub_router};
use super::last_known_good;
use super::launch::AgentLauncher;
use super::overview::{OverviewTracker, TeamOverview};
use super::push::{PushService, PushTriggers, TriggerInputs};
use super::relay_agents::RelayAgents;
use super::services::{HubControl, HubServices};
use super::team_embedding::EmbeddingSource;
use super::team_events::{TeamEventLog, TeamEventRecorder, record_hub_started};
use super::types::{HubEvent, HubNoticeKind, NoticeLevel};
use super::web::WebAssetSource;

/// How often the hub checks for a newer version.
const UPDATE_CHECK_INTERVAL: Duration = Duration::from_hours(6);

/// What the process hands the hub to run with.
pub struct HubInputs {
    /// The residuum root, which holds `hub/` and one directory per agent.
    pub root: PathBuf,
    /// The version and commit of the running binary.
    pub build: BuildInfo,
    /// Starts each agent's runtime.
    pub launcher: Arc<dyn AgentLauncher>,
    /// The embedded web app, which the static routes and the MCP catalog serve.
    pub assets: WebAssetSource,
    /// Called once the servers are bound and the autostart agents have started
    /// or failed; the process uses it to write its readiness marker.
    pub on_ready: Box<dyn FnOnce(&Path) + Send>,
    /// The global log filter, which a `[tracing]` reload changes.
    pub log_filter: Option<&'static dyn FilterReload>,
}

/// Runtime log level switching.
///
/// Type-erases the `tracing_subscriber::reload::Handle` so it can be stored
/// globally without naming the full subscriber stack type.
pub trait FilterReload: Send + Sync {
    /// Replace the active `EnvFilter` with one for the given log level.
    ///
    /// # Errors
    /// Returns an error if the filter cannot be applied.
    fn set_filter(&self, level: LogLevel) -> Result<(), String>;
}

/// Outcome of the hub's main loop.
pub enum HubExit {
    /// Clean shutdown (inbound channel closed).
    Shutdown,
    /// Restart requested (binary updated, re-exec needed).
    Restart,
}

/// Platform-aware termination signal.
///
/// On Unix, wraps a SIGTERM listener. On Windows (and other platforms), `recv()`
/// pends forever — graceful shutdown is handled via the HTTP `/api/hub/shutdown` endpoint
/// or the cross-platform Ctrl+C handler instead.
struct TermSignal {
    #[cfg(unix)]
    inner: tokio::signal::unix::Signal,
}

impl TermSignal {
    /// Register the SIGTERM listener.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the signal handler cannot be registered.
    #[cfg(unix)]
    fn new() -> std::io::Result<Self> {
        let inner = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        Ok(Self { inner })
    }

    /// Create the termination signal. Nothing to register on this platform.
    #[cfg(not(unix))]
    #[must_use]
    fn new() -> Self {
        Self {}
    }

    /// Wait for the termination signal. On non-Unix platforms this never resolves.
    #[cfg(unix)]
    async fn recv(&mut self) {
        self.inner.recv().await;
    }

    /// Wait for the termination signal. On non-Unix platforms this never resolves.
    #[cfg(not(unix))]
    async fn recv(&mut self) {
        std::future::pending::<()>().await;
    }
}

/// A running HTTP server task and the switch that stops it.
struct HttpServer {
    handle: JoinHandle<()>,
    shutdown_tx: watch::Sender<bool>,
}

/// The relay tunnel task and the switch that stops it.
struct TunnelTask {
    handle: JoinHandle<()>,
    shutdown_tx: watch::Sender<bool>,
}

/// The hub's A2A listener task and the switch that stops it.
struct A2aListenerTask {
    handle: JoinHandle<()>,
    shutdown_tx: watch::Sender<bool>,
}

/// Register the termination signal the run loop waits on. Only Unix can fail
/// to register it.
#[cfg_attr(
    not(unix),
    expect(
        clippy::unnecessary_wraps,
        reason = "registering the termination signal can fail only on Unix"
    )
)]
fn register_term_signal() -> Result<TermSignal, FatalError> {
    #[cfg(unix)]
    let sigterm = TermSignal::new()
        .map_err(|e| FatalError::Gateway(format!("failed to register termination handler: {e}")))?;
    #[cfg(not(unix))]
    let sigterm = TermSignal::new();
    Ok(sigterm)
}

/// Load the hub config, falling back to its last-known-good copy if the live
/// file fails to load. Returns the hub config actually used and — only when
/// the fallback was used — a description of what was wrong with the live
/// file.
fn load_hub_with_fallback(hub_dir: &Path) -> Result<(HubConfig, Option<String>), FatalError> {
    let Some(root) = hub_dir.parent() else {
        return Err(FatalError::Config(format!(
            "hub directory {} has no parent, so agents can't be discovered",
            hub_dir.display()
        )));
    };
    match HubConfig::load_at_for_start(hub_dir, root) {
        Ok(hub) => Ok((hub, None)),
        Err(err) => match last_known_good::hub::load(hub_dir) {
            Ok(hub) => Ok((hub, Some(err.to_string()))),
            Err(_) => Err(err),
        },
    }
}

/// What the hub learns about its agents before any of them starts.
struct AgentScan {
    /// Teams adapter ports the agents are configured for, which the
    /// workbench listener must stay off.
    teams_ports: Vec<u16>,
    /// The embedding model the team wiki uses: the first agent (by name) with
    /// one configured.
    team_embedding: Option<EmbeddingSource>,
}

/// Load every agent's config, best effort, for the settings that are shared
/// across the hub. An agent whose config doesn't load is left out here; it
/// reports its own problem when it starts.
fn scan_agents(root: &Path, hub: &HubConfig) -> AgentScan {
    let mut scan = AgentScan {
        teams_ports: Vec::new(),
        team_embedding: None,
    };
    let names = match residuum_config::discover_agents(root) {
        Ok(names) => names,
        Err(e) => {
            tracing::warn!(error = %e, "couldn't scan for agents before start-up");
            return scan;
        }
    };
    for name in names {
        let agent_dir = residuum_config::paths::agent_dir(root, &name);
        let cfg = match Config::load_agent_at(&agent_dir, hub) {
            Ok(cfg) => cfg,
            Err(e) => {
                tracing::debug!(agent = %name, error = %e, "skipping an agent whose config doesn't load while scanning");
                continue;
            }
        };
        if let Some(teams) = &cfg.teams {
            scan.teams_ports.push(teams.port);
        }
        if scan.team_embedding.is_none() {
            scan.team_embedding = EmbeddingSource::from_config(&name, &cfg);
        }
    }
    scan
}

/// Build the provider for the team wiki's embedding model. A provider that
/// can't be built leaves wiki search text only.
fn build_team_embedding(source: &EmbeddingSource) -> Option<Arc<dyn EmbeddingProvider>> {
    match source.build() {
        Ok(provider) => Some(provider),
        Err(e) => {
            tracing::warn!(agent = %source.agent(), error = %e, "the team wiki's embedding provider is unavailable; wiki search is text only");
            None
        }
    }
}

/// Bind the gateway's address from its config. The hub serves on the bound
/// listener, so binding happens before the hub's other services start.
///
/// # Errors
/// Returns `FatalError::Gateway` if the address cannot be bound.
async fn bind_gateway(
    gateway: &residuum_config::GatewayConfig,
) -> Result<tokio::net::TcpListener, FatalError> {
    let addr = gateway.addr();
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| FatalError::Gateway(format!("failed to bind to {addr}: {e}")))?;
    tracing::info!(addr = %addr, "gateway listening");
    if gateway.bind != "127.0.0.1" && gateway.bind != "localhost" {
        tracing::warn!(
            bind = %gateway.bind,
            "web UI is exposed on a non-loopback address with no authentication"
        );
    }
    Ok(listener)
}

/// Serve `app` on the gateway's bound `listener`.
fn spawn_http_server(listener: tokio::net::TcpListener, app: axum::Router) -> HttpServer {
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
    let handle = residuum_core::util::spawn_in_span(async move {
        if let Err(e) = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                shutdown_rx.wait_for(|stop| *stop).await.ok();
            })
            .await
        {
            tracing::error!(error = %e, "gateway server error");
        }
    });
    HttpServer {
        handle,
        shutdown_tx,
    }
}

/// What the secure tunnel serves and reports through, kept for the hub's
/// lifetime and handed to each tunnel task.
#[derive(Clone)]
struct RemoteWiring {
    /// The main gateway router, served in-process for the UI host.
    ui_router: axum::Router,
    /// The workbench router, served in-process for the workbench host.
    workbench_router: axum::Router,
    pairing: residuum_net::pairing::DevicePairing,
    slot: residuum_net::remote_access::slot::RemoteAccessSlot,
    /// Delivers a warning to the user as a hub notice.
    notify: residuum_net::remote_access::manager::Notifier,
}

impl RemoteWiring {
    fn new(
        app: &axum::Router,
        workbench_router: Option<axum::Router>,
        services: &HubServices,
        host: &Arc<AgentHost>,
    ) -> Self {
        let notices = Arc::clone(host);
        Self {
            ui_router: app.clone(),
            workbench_router: workbench_router.unwrap_or_else(|| {
                residuum_net::workbench::unavailable_router(services.pairing.clone())
            }),
            pairing: services.pairing.clone(),
            slot: services.remote_access.clone(),
            notify: Arc::new(move |message| notices.notice(NoticeLevel::Warn, message, None)),
        }
    }
}

/// The handles a relay tunnel is started with.
struct TunnelInputs<'a> {
    remote: &'a RemoteWiring,
    status_tx: &'a Arc<watch::Sender<TunnelStatus>>,
    relay_agents: &'a RelayAgents,
    teams_ports: watch::Receiver<std::collections::BTreeMap<String, u16>>,
}

/// Start the relay tunnel for `cloud`, which keeps the relay's agent list
/// current from `relay_agents`. `None` when the secure tunnel's manager can't
/// be built (logged, and reported through the remote access status).
fn spawn_tunnel(
    hub: &HubConfig,
    cloud: &residuum_config::CloudConfig,
    inputs: &TunnelInputs<'_>,
) -> Option<TunnelTask> {
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let wiring = inputs.remote;
    let tunnel = wiring
        .slot
        .start_tunnel(residuum_net::remote_access::slot::TunnelStart {
            cloud: cloud.clone(),
            hub_dir: hub.config_dir.clone(),
            a2a_port: hub.a2a.enabled.then_some(hub.a2a.port),
            ui_router: wiring.ui_router.clone(),
            workbench_router: wiring.workbench_router.clone(),
            teams_ports: inputs.teams_ports.clone(),
            pairing: wiring.pairing.clone(),
            tunnel_status: Arc::clone(inputs.status_tx),
            notify: Arc::clone(&wiring.notify),
            agents: inputs.relay_agents.subscribe(),
            shutdown: shutdown_rx,
        })?;
    let handle = residuum_core::util::spawn_monitored("tunnel", tunnel);
    Some(TunnelTask {
        handle,
        shutdown_tx,
    })
}

/// Start the tunnel the hub config asks for at startup, if it asks for one.
fn spawn_initial_tunnel(hub: &HubConfig, inputs: &TunnelInputs<'_>) -> Option<TunnelTask> {
    hub.cloud
        .as_ref()
        .and_then(|cloud| spawn_tunnel(hub, cloud, inputs))
}

/// Bind the A2A listener's address from its config, when A2A is enabled. A
/// bind that fails is logged and left to the listener, which tries again when
/// it starts.
async fn bind_a2a(hub: &HubConfig) -> Option<tokio::net::TcpListener> {
    if !hub.a2a.enabled {
        return None;
    }
    let addr = format!("{}:{}", hub.gateway.bind, hub.a2a.port);
    match tokio::net::TcpListener::bind(&addr).await {
        Ok(listener) => Some(listener),
        Err(e) => {
            tracing::warn!(addr = %addr, error = %e, "a2a interface couldn't bind; it will try again when it starts");
            None
        }
    }
}

/// Start the hub's A2A listener over `host`, serving every agent under
/// `/agents/{name}/`. It serves on `bound` when the caller bound it.
fn spawn_a2a_listener(
    hub: &HubConfig,
    host: &Arc<AgentHost>,
    services: &HubServices,
    bound: Option<tokio::net::TcpListener>,
) -> A2aListenerTask {
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    // Nothing the relay says names a caller: siblings present the keys they
    // were issued when they joined.
    let mut listener = residuum_a2a::A2aListener::new(
        hub.gateway.bind.clone(),
        hub.a2a.port,
        Arc::clone(host) as Arc<dyn AgentDirectory>,
        Arc::clone(&services.a2a_keys),
        shutdown_rx,
    )
    .with_sibling_keys(services.remote_access.sibling_keys());
    if let Some(bound) = bound {
        listener = listener.with_bound_listener(bound);
    }
    let host = Arc::clone(host);
    let port = hub.a2a.port;
    let handle = residuum_core::util::spawn_monitored("a2a", async move {
        if let Err(e) = listener.start().await {
            tracing::error!(error = %e, port, "a2a interface failed");
            host.notice(NoticeLevel::Warn, a2a_failure_message(port, &e), None);
        }
    });
    A2aListenerTask {
        handle,
        shutdown_tx,
    }
}

/// The user's notice for an A2A listener that could not serve on `port`. The
/// reason comes from the io error under `error`, put in plain words.
fn a2a_failure_message(port: u16, error: &anyhow::Error) -> String {
    let reason = match error
        .chain()
        .find_map(|cause| cause.downcast_ref::<std::io::Error>())
        .map(std::io::Error::kind)
    {
        Some(std::io::ErrorKind::AddrInUse) => {
            "the port is already in use. Choose another port in settings or stop the program using it."
        }
        Some(std::io::ErrorKind::PermissionDenied) => {
            "the system doesn't allow residuum to use that port. Choose another port in settings."
        }
        _ => "something went wrong while starting it. Check the logs for details.",
    };
    format!("The agent-to-agent listener couldn't start on port {port}: {reason}")
}

/// Stop a task by its switch and wait up to five seconds for it to finish.
async fn stop_task(name: &str, shutdown_tx: &watch::Sender<bool>, handle: JoinHandle<()>) {
    shutdown_tx.send(true).ok();
    if tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .is_err()
    {
        tracing::warn!(task = %name, "task didn't stop within 5s");
    }
}

/// What the hub tells its clients about the team, and what keeps it current:
/// the event log, the overview and the Web Push notifications, each fed from
/// the host's events and from the feed of agent changes for as long as this
/// lives.
struct TeamTracking {
    /// What has happened across the team since this process started.
    events: Arc<TeamEventLog>,
    /// What Home shows about each agent.
    overview: Arc<TeamOverview>,
    _recorder: TeamEventRecorder,
    _tracker: OverviewTracker,
    _push_triggers: PushTriggers,
}

impl TeamTracking {
    /// Start tracking `host`. The log is the process's: its boot id is the
    /// one the hub socket announces, and the overview names it too. `push`
    /// sends the notifications that what the host reports calls for.
    fn start(host: &Arc<AgentHost>, push: &Arc<PushService>) -> Self {
        let events = TeamEventLog::new(uuid::Uuid::new_v4().to_string());
        record_hub_started(&events);
        let recorder = TeamEventRecorder::spawn(
            Arc::clone(&events),
            host.subscribe(),
            host.agent_changes().subscribe(),
        );
        let overview = TeamOverview::new(
            events.boot_id(),
            Arc::clone(host) as Arc<dyn AgentDirectory>,
        );
        let tracker = OverviewTracker::spawn(
            Arc::clone(&overview),
            host.subscribe(),
            host.agent_changes().subscribe(),
        );
        let notices = Arc::clone(host);
        let push_triggers = PushTriggers::spawn(TriggerInputs {
            push: Arc::clone(push),
            directory: Arc::clone(host) as Arc<dyn AgentDirectory>,
            hub_events: host.subscribe(),
            changes: host.agent_changes().subscribe(),
            notice: Box::new(move |message| notices.notice(NoticeLevel::Warn, message, None)),
            decisions: None,
        });
        Self {
            events,
            overview,
            _recorder: recorder,
            _tracker: tracker,
            _push_triggers: push_triggers,
        }
    }
}

/// The hub's process-level state.
pub struct HubRuntime {
    hub_dir: std::path::PathBuf,
    hub_cfg: HubConfig,
    services: HubServices,
    host: Arc<AgentHost>,
    /// Records what happens across the team and keeps every agent's
    /// overview current, for as long as the hub runs.
    _team_tracking: TeamTracking,
    app: axum::Router,
    server: HttpServer,
    /// Resolves when onboarding has written the first agent; `None` once the
    /// hub has agents (or from the start, if it began with some).
    setup_done_rx: Option<watch::Receiver<bool>>,
    tunnel: Option<TunnelTask>,
    tunnel_status_tx: Arc<watch::Sender<TunnelStatus>>,
    remote_wiring: RemoteWiring,
    relay_agents: RelayAgents,
    a2a: Option<A2aListenerTask>,
    workbench_shutdown_tx: Option<watch::Sender<bool>>,
    reload_rx: ReloadReceiver,
    watcher: JoinHandle<()>,
    restart_rx: mpsc::Receiver<()>,
    shutdown_rx: mpsc::Receiver<()>,
    sigterm: TermSignal,
    log_filter: Option<&'static dyn FilterReload>,
}

impl HubRuntime {
    /// The services every hosted agent receives.
    #[must_use]
    pub fn services(&self) -> &HubServices {
        &self.services
    }

    /// The host that runs the hub's agents.
    #[must_use]
    pub fn host(&self) -> &Arc<AgentHost> {
        &self.host
    }

    /// Start the hub on the residuum root in `inputs`: build the shared services
    /// and the host, bring up the servers, and start the `autostart` agents.
    ///
    /// # Errors
    /// A [`FatalError`] when the shared services or the servers cannot start.
    pub async fn start(
        inputs: HubInputs,
        hub_cfg: HubConfig,
        fallback_problem: Option<String>,
        gateway: tokio::net::TcpListener,
        a2a_listener: Option<tokio::net::TcpListener>,
    ) -> Result<Self, FatalError> {
        let HubInputs {
            root,
            build,
            launcher,
            assets,
            on_ready,
            log_filter,
        } = inputs;
        let hub_dir = hub_cfg.config_dir.clone();
        let (restart_tx, restart_rx) = mpsc::channel::<()>(1);
        let (shutdown_tx, shutdown_rx) = mpsc::channel::<()>(1);
        let (tunnel_status_tx, tunnel_status_rx) = watch::channel(TunnelStatus::Disconnected);
        let tunnel_status_tx = Arc::new(tunnel_status_tx);

        let pairing = residuum_net::pairing::DevicePairing::open(&hub_dir);
        pairing.track_identity(tunnel_status_rx.clone());

        let scan = scan_agents(&root, &hub_cfg);
        let (workbench_serving, workbench_shutdown_tx, workbench_api, workbench_router) =
            start_workbench_listener(&root, build, &hub_cfg, &scan.teams_ports, &pairing).await;
        let services = HubServices::open(
            &root,
            &hub_cfg,
            tunnel_status_rx,
            HubControl::new(build, restart_tx, shutdown_tx),
            workbench_serving,
            pairing,
            scan.team_embedding.as_ref().and_then(build_team_embedding),
        )
        .await?;

        let host = AgentHost::new(services.clone(), hub_cfg.clone(), launcher);
        // Tracking comes first, so the hub's startup notices and every
        // agent's start reach the log, the overview and the push triggers:
        // nothing they listen to is replayed.
        let team = TeamTracking::start(&host, &services.push);
        host.note_team_embedding(scan.team_embedding).await;
        let agents = host.discover()?;
        tracing::info!(agents = agents.len(), names = %agents.join(", "), "found agents");
        publish_startup_notices(&host, &hub_cfg, fallback_problem.as_deref());

        let (reload_tx, reload_rx) = tokio::sync::mpsc::unbounded_channel();
        // With no agents the web app runs onboarding, whose last step signals
        // this once the first agent is on disk.
        let setup_done = agents.is_empty().then(|| Arc::new(watch::channel(false).0));
        let setup_done_rx = setup_done.as_ref().map(|tx| tx.subscribe());
        let app = build_app(
            &host,
            &services,
            &team.events,
            &team.overview,
            reload_tx.clone(),
            setup_done,
            assets,
        )?;
        workbench_api.bind(app.clone());
        let remote_wiring = RemoteWiring::new(&app, workbench_router, &services, &host);
        let server = spawn_http_server(gateway, app.clone());
        let a2a = hub_cfg
            .a2a
            .enabled
            .then(|| spawn_a2a_listener(&hub_cfg, &host, &services, a2a_listener));
        let relay_agents = RelayAgents::spawn(
            Arc::clone(&host) as Arc<dyn AgentDirectory>,
            hub_cfg.a2a.enabled,
        );
        let tunnel = spawn_initial_tunnel(
            &hub_cfg,
            &TunnelInputs {
                remote: &remote_wiring,
                status_tx: &tunnel_status_tx,
                relay_agents: &relay_agents,
                teams_ports: host.subscribe_teams_ports(),
            },
        );
        let watcher =
            config_watcher::spawn_hub_config_watcher(hub_dir.join("config.toml"), reload_tx);
        let sigterm = register_term_signal()?;

        host.start_autostart().await;

        // Reaching this point means the servers are bound and every
        // autostart agent has either started or been recorded as failed.
        on_ready(&hub_dir);
        if fallback_problem.is_none() {
            last_known_good::hub::save(&hub_dir);
        }

        Ok(Self {
            hub_dir,
            hub_cfg,
            services,
            host,
            _team_tracking: team,
            app,
            server,
            setup_done_rx,
            tunnel,
            tunnel_status_tx,
            remote_wiring,
            relay_agents,
            a2a,
            workbench_shutdown_tx,
            reload_rx,
            watcher,
            restart_rx,
            shutdown_rx,
            sigterm,
            log_filter,
        })
    }

    /// Run until told to stop, then stop every agent and the servers.
    /// `check_updates` turns the periodic version check on.
    pub async fn run(mut self, check_updates: bool) -> HubExit {
        if check_updates {
            spawn_update_check(&self.services.control.update_status);
        }
        let mut update_tick = tokio::time::interval(UPDATE_CHECK_INTERVAL);
        update_tick.tick().await; // the check above covers the first tick
        tracing::info!("hub ready");

        let exit = loop {
            tokio::select! {
                () = self.sigterm.recv() => {
                    tracing::info!("received SIGTERM, shutting down");
                    break HubExit::Shutdown;
                }
                _ = self.shutdown_rx.recv() => {
                    tracing::info!("shutdown requested through the HTTP API");
                    break HubExit::Shutdown;
                }
                _ = self.restart_rx.recv() => {
                    tracing::info!("restart requested, shutting down for re-exec");
                    break HubExit::Restart;
                }
                signal = self.reload_rx.recv() => {
                    if let Some(first) = signal {
                        self.handle_reload_signals(first).await;
                    }
                }
                () = setup_finished(&mut self.setup_done_rx) => {
                    self.start_first_agents().await;
                }
                _ = update_tick.tick(), if check_updates => {
                    tracing::debug!("scheduled update check triggered");
                    spawn_update_check(&self.services.control.update_status);
                }
                result = poll_tunnel(&mut self.tunnel) => {
                    self.respawn_tunnel(&result).await;
                }
            }
        };
        self.shut_down().await;
        exit
    }

    /// Stop every agent, then the servers.
    async fn shut_down(mut self) {
        tracing::info!("beginning hub shutdown");
        // Refuse starts before stopping agents, so nothing can start after
        // the sweep and be left running when the servers go down.
        self.host.begin_shutdown();
        self.host.stop_all().await;
        self.watcher.abort();
        self.relay_agents.stop();
        if let Some(tunnel) = self.tunnel.take() {
            stop_task("tunnel", &tunnel.shutdown_tx, tunnel.handle).await;
        }
        if let Some(a2a) = self.a2a.take() {
            stop_task("a2a", &a2a.shutdown_tx, a2a.handle).await;
        }
        if let Some(tx) = self.workbench_shutdown_tx.take() {
            tx.send(true).ok();
        }
        stop_task("http", &self.server.shutdown_tx, self.server.handle).await;
        tracing::info!("hub shutdown complete");
    }

    /// Log the tunnel task's unexpected exit, auto-report it, and respawn it.
    async fn respawn_tunnel(&mut self, exit: &Result<(), tokio::task::JoinError>) {
        let described = match exit {
            Ok(()) => {
                tracing::error!("tunnel task exited unexpectedly, attempting respawn");
                "tunnel task exited unexpectedly".to_string()
            }
            Err(e) => {
                tracing::error!(error = %e, "tunnel task failed, attempting respawn");
                format!("tunnel task failed: {e}")
            }
        };
        self.services
            .tracing_service
            .on_error(
                &described,
                residuum_tracing_service::client_context::gather_for_hub(
                    self.services.control.build,
                ),
            )
            .await;
        self.tunnel_status_tx.send(TunnelStatus::Disconnected).ok();
        if let Some(cloud) = &self.hub_cfg.cloud {
            self.tunnel = spawn_tunnel(
                &self.hub_cfg,
                cloud,
                &TunnelInputs {
                    remote: &self.remote_wiring,
                    status_tx: &self.tunnel_status_tx,
                    relay_agents: &self.relay_agents,
                    teams_ports: self.host.subscribe_teams_ports(),
                },
            );
            if self.tunnel.is_some() {
                tracing::info!("tunnel respawned after unexpected exit");
            }
        } else {
            self.tunnel = None;
        }
    }

    /// Act on the reload signals queued behind `first`: one hub reload
    /// covers every hub config edit, and one workspace signal per running
    /// agent covers every team identity edit.
    async fn handle_reload_signals(&mut self, first: ReloadSignal) {
        let mut hub_changed = false;
        let mut team_changed = false;
        let mut note = |signal: ReloadSignal| match signal {
            ReloadSignal::Workspace => team_changed = true,
            // Nothing sends an agent-scoped signal to the hub; one that
            // arrives is read as a hub reload so it isn't dropped.
            ReloadSignal::Hub | ReloadSignal::Agent => hub_changed = true,
        };
        note(first);
        while let Ok(next) = self.reload_rx.try_recv() {
            note(next);
        }
        if hub_changed {
            self.reload().await;
        }
        if team_changed {
            self.host.team_files_changed();
        }
    }

    /// Onboarding wrote the first agent: apply the hub config it wrote, find
    /// the agent, and start it.
    async fn start_first_agents(&mut self) {
        self.reload().await;
        match self.host.discover() {
            Ok(agents) => {
                tracing::info!(names = %agents.join(", "), "onboarding finished; starting the first agent");
            }
            Err(e) => {
                tracing::error!(error = %e, "couldn't look for the agent onboarding created");
                self.host.notice(
                    NoticeLevel::Error,
                    format!(
                        "Setup finished, but residuum couldn't find the new agent ({e}). Restart residuum to start it."
                    ),
                    None,
                );
                return;
            }
        }
        self.host.start_autostart().await;
    }

    /// Reload `hub/config.toml` in place.
    ///
    /// A config that fails to load keeps the running one in effect. What
    /// changed is applied here where the hub owns it (the HTTP address, the
    /// relay tunnel, tracing, the A2A listener), and every running agent is
    /// told to reload against the new config for the rest.
    async fn reload(&mut self) {
        tracing::info!("handling hub config reload");
        let new_hub = match HubConfig::load_at(&self.hub_dir) {
            Ok(hub) => hub,
            Err(err) => {
                tracing::warn!(error = %err, "hub config reload failed, keeping current hub config");
                self.host.publish(HubEvent::HubConfigReloaded {
                    ok: false,
                    changed: false,
                    message: Some(format!(
                        "hub config reload failed (keeping current hub config): {err}"
                    )),
                });
                return;
            }
        };
        for notice in &new_hub.load_notices {
            self.host.notice(NoticeLevel::Warn, notice.clone(), None);
        }
        if new_hub == self.hub_cfg {
            last_known_good::hub::save(&self.hub_dir);
            tracing::info!("hub config reload: no changes detected");
            self.host.publish(HubEvent::HubConfigReloaded {
                ok: true,
                changed: false,
                message: None,
            });
            return;
        }

        let old = std::mem::replace(&mut self.hub_cfg, new_hub.clone());
        let mut changed = Vec::new();
        if old.gateway != new_hub.gateway {
            changed.push("gateway bind/port");
            self.rebind_http(&new_hub).await;
        }
        if old.tracing != new_hub.tracing {
            changed.push("tracing");
            self.apply_tracing(&new_hub).await;
        }
        self.relay_agents
            .set_a2a_listener_enabled(new_hub.a2a.enabled);
        if old.cloud != new_hub.cloud || old.a2a != new_hub.a2a {
            changed.push("cloud");
            self.restart_tunnel(&new_hub).await;
        }
        if old.a2a != new_hub.a2a || old.gateway.bind != new_hub.gateway.bind {
            changed.push("a2a");
            self.restart_a2a_listener(&new_hub).await;
        }
        if old.timezone != new_hub.timezone {
            changed.push("timezone");
        }
        if old.push != new_hub.push {
            changed.push("push");
            self.services
                .push
                .set_contact(new_hub.push.contact.as_deref());
        }
        if old.system_one != new_hub.system_one {
            changed.push("decision model");
            self.services
                .system_one
                .reconfigure(new_hub.system_one.as_ref());
        }
        if old.background.max_concurrent != new_hub.background.max_concurrent {
            changed.push("background limits");
            self.host.notice_of_kind(
                NoticeLevel::Info,
                HubNoticeKind::BackgroundLimitAfterRestart,
                "background.max_concurrent changed in hub/config.toml — this takes effect on the next restart, not immediately.".to_string(),
            );
        } else if old.background != new_hub.background {
            changed.push("background limits");
        }

        self.host.hub_config_changed(new_hub);
        last_known_good::hub::save(&self.hub_dir);
        let summary = changed.join(", ");
        tracing::info!(changes = %summary, "hub configuration reloaded successfully");
        self.host.publish(HubEvent::HubConfigReloaded {
            ok: true,
            changed: true,
            message: Some(format!("hub configuration reloaded: {summary}")),
        });
    }

    /// Serve the hub's HTTP app on the new address, then retire the old
    /// server. A bind failure keeps the current server and tells the user.
    async fn rebind_http(&mut self, new_hub: &HubConfig) {
        let listener = match bind_gateway(&new_hub.gateway).await {
            Ok(listener) => listener,
            Err(e) => {
                tracing::warn!(error = %e, "failed to bind to the new gateway address, keeping the current server");
                self.host.notice_of_kind(
                    NoticeLevel::Warn,
                    HubNoticeKind::GatewayRebindFailed,
                    format!(
                        "gateway rebind failed ({}): {e} — keeping the current server",
                        new_hub.gateway.addr()
                    ),
                );
                return;
            }
        };
        let server = spawn_http_server(listener, self.app.clone());
        let old = std::mem::replace(&mut self.server, server);
        old.shutdown_tx.send(true).ok();
        tracing::info!(addr = %new_hub.gateway.addr(), "gateway rebound to new address");
    }

    /// Push a changed `[tracing]` section into the tracing service and the
    /// global log filter.
    async fn apply_tracing(&self, new_hub: &HubConfig) {
        self.services
            .tracing_service
            .update_config(new_hub.tracing.clone())
            .await;
        if let Some(handle) = self.log_filter
            && let Err(e) = handle.set_filter(new_hub.tracing.log_level)
        {
            tracing::warn!(error = %e, "failed to update the log filter on a tracing config reload");
        }
        tracing::debug!(level = %new_hub.tracing.log_level, "tracing config updated");
    }

    /// Stop the tunnel (if running) and start one for the new config.
    async fn restart_tunnel(&mut self, new_hub: &HubConfig) {
        if let Some(tunnel) = self.tunnel.take() {
            stop_task("tunnel", &tunnel.shutdown_tx, tunnel.handle).await;
        }
        self.tunnel_status_tx.send(TunnelStatus::Disconnected).ok();
        if let Some(cloud) = &new_hub.cloud {
            self.tunnel = spawn_tunnel(
                new_hub,
                cloud,
                &TunnelInputs {
                    remote: &self.remote_wiring,
                    status_tx: &self.tunnel_status_tx,
                    relay_agents: &self.relay_agents,
                    teams_ports: self.host.subscribe_teams_ports(),
                },
            );
            if self.tunnel.is_some() {
                tracing::info!("tunnel restarted with new config");
            }
        } else {
            self.remote_wiring.slot.disable();
            tracing::info!("cloud tunnel removed from config");
        }
    }

    /// Stop the A2A listener (if running) and start one for the new config.
    async fn restart_a2a_listener(&mut self, new_hub: &HubConfig) {
        if let Some(a2a) = self.a2a.take() {
            stop_task("a2a", &a2a.shutdown_tx, a2a.handle).await;
        }
        if new_hub.a2a.enabled {
            self.a2a = Some(spawn_a2a_listener(
                new_hub,
                &self.host,
                &self.services,
                None,
            ));
            tracing::info!("a2a listener restarted with new config");
        } else {
            tracing::info!("a2a listener removed from config");
        }
    }
}

/// Resolves once onboarding signals that the first agent is written, then
/// clears the slot; pends forever when there is nothing to wait for.
async fn setup_finished(setup_done_rx: &mut Option<watch::Receiver<bool>>) {
    let Some(rx) = setup_done_rx else {
        return std::future::pending().await;
    };
    if rx.wait_for(|done| *done).await.is_err() {
        // Every sender is gone, so no client can finish onboarding.
        *setup_done_rx = None;
        return std::future::pending().await;
    }
    *setup_done_rx = None;
}

/// Build the hub's HTTP app over `host`, with the process-wide handles from
/// `services`, the team event log `team_events`, whose boot id the hub socket
/// announces, and the team `overview`.
///
/// # Errors
/// Returns `FatalError::Gateway` if the hub-level checkpoint repositories
/// can't be opened.
pub fn build_app(
    host: &Arc<AgentHost>,
    services: &HubServices,
    team_events: &Arc<TeamEventLog>,
    overview: &Arc<TeamOverview>,
    reload_tx: residuum_bus::control::ReloadSender,
    setup_done: Option<Arc<watch::Sender<bool>>>,
    assets: WebAssetSource,
) -> Result<axum::Router, FatalError> {
    let checkpoints = residuum_checkpoints::CheckpointEngine::hub_scoped(
        Arc::clone(&services.checkpoints),
        &services.hub_dir,
    )
    .map_err(|e| FatalError::Gateway(format!("failed to open the hub's checkpoints: {e}")))?
    .with_team_coordinator(services.team.clone());
    let subagents_host = Arc::clone(host);
    let state = HubHttpState {
        hub_dir: services.hub_dir.clone(),
        reload_tx,
        setup_done,
        assets,
        secret_lock: Arc::clone(&services.secret_lock),
        checkpoints: Arc::new(checkpoints),
        tunnel_status_rx: services.tunnel_status_rx.clone(),
        build: services.control.build,
        update_status: Arc::clone(&services.control.update_status),
        restart_tx: services.control.restart_tx.clone(),
        shutdown_tx: services.control.shutdown_tx.clone(),
        tracing_service: Arc::clone(&services.tracing_service),
        client_context: Arc::new(residuum_tracing_service::client_context::gather_for_hub(
            services.control.build,
        )),
        active_subagents: Arc::new(move || subagents_host.active_subagents()),
        workbench_serving: services.workbench_serving.clone(),
        team: services.team.clone(),
        team_bus: services.team_feed.bus.clone(),
        team_watch_health: services.team_feed.health.clone(),
        started_at: std::time::Instant::now(),
        push: Arc::clone(&services.push),
        team_events: Arc::clone(team_events),
        overview: Arc::clone(overview),
        agent_changes: Arc::clone(host.agent_changes()),
        system_one: Arc::clone(&services.system_one),
        pairing: services.pairing.clone(),
        remote_access: services.remote_access.clone(),
        uploads: services.uploads.clone(),
    };
    Ok(hub_router(
        Arc::clone(host) as Arc<dyn AgentDirectory>,
        state,
    ))
}

/// Await the tunnel task if there is one, clearing the slot when it ends;
/// pends forever otherwise.
async fn poll_tunnel(tunnel: &mut Option<TunnelTask>) -> Result<(), tokio::task::JoinError> {
    match tunnel {
        Some(task) => {
            let result = (&mut task.handle).await;
            *tunnel = None;
            result
        }
        None => std::future::pending().await,
    }
}

/// Start the workbench artifacts listener beside the gateway. Teams' and
/// A2A's configured ports stay free for them, and so do their defaults, so
/// enabling either later can't collide with the artifacts listener.
///
/// The listener forwards `/api` to the hub router, which needs the listener's
/// port and so is built after it. The returned handle is where the caller
/// binds that router once it exists.
async fn start_workbench_listener(
    root: &Path,
    build: BuildInfo,
    hub: &HubConfig,
    teams_ports: &[u16],
    pairing: &residuum_net::pairing::DevicePairing,
) -> (
    residuum_net::workbench::WorkbenchServing,
    Option<watch::Sender<bool>>,
    HubApi,
    Option<axum::Router>,
) {
    let mut reserved = vec![
        residuum_config::DEFAULT_TEAMS_PORT,
        hub.a2a.port,
        residuum_config::DEFAULT_A2A_PORT,
    ];
    reserved.extend_from_slice(teams_ports);
    let workbench_dir =
        residuum_config::paths::TeamPaths::new(residuum_config::paths::team_dir(root))
            .workbench_dir();
    let api = HubApi::new();
    let (serving, shutdown_tx, router) = residuum_net::workbench::start(
        &hub.gateway.bind,
        hub.gateway.port,
        &reserved,
        workbench_dir,
        api.clone(),
        pairing.clone(),
        workbench_sdk(build),
    )
    .await;
    (serving, shutdown_tx, api, router)
}

/// The SDK the artifacts listener injects into every served page: this
/// build's version and the features it supports.
fn workbench_sdk(build: BuildInfo) -> SdkContext {
    SdkContext {
        version: build.version,
        features: crate::features::FEATURES,
    }
}

/// Report what the hub itself found wrong while starting: a fallback to its
/// last-known-good config, load notices, and removed environment overrides.
fn publish_startup_notices(host: &AgentHost, hub: &HubConfig, fallback_problem: Option<&str>) {
    if let Some(problem) = fallback_problem {
        tracing::error!(error = %problem, "hub startup fell back to the last-known-good config");
        host.notice(
            NoticeLevel::Error,
            format!(
                "residuum couldn't start using your current hub config ({problem}). It's running on the last configuration that worked instead — fix hub/config.toml, then reload (or restart residuum) to apply your changes."
            ),
            None,
        );
    }
    for notice in &hub.load_notices {
        tracing::warn!(%notice, "hub config notice");
        host.notice(NoticeLevel::Warn, notice.clone(), None);
    }
    for notice in residuum_config::removed_agent_env_override_notices() {
        tracing::warn!(%notice, "removed environment override is still set");
        host.notice(NoticeLevel::Warn, notice, None);
    }
}

/// Spawn a fire-and-forget update check task.
fn spawn_update_check(status: &crate::update::SharedUpdateStatus) {
    let status = Arc::clone(status);
    residuum_core::util::spawn_monitored("update-check", async move {
        crate::update::check_for_update(&status).await;
    });
}

/// Run the hub on the residuum root in `inputs` until it is told to stop.
///
/// Loads the hub config (falling back to its last-known-good copy; a missing
/// timezone is allowed when no agent exists, so onboarding can ask for one),
/// starts the servers and the shared services, and starts every agent whose
/// `autostart` is on. Returns [`HubExit::Restart`] when the binary was
/// updated and the process should relaunch.
///
/// # Errors
///
/// Returns `FatalError` if the hub config can't be loaded and has no working
/// last-known-good copy, or a hub-level service (the HTTP server, the team
/// wiki index, the checkpoint repositories) cannot start. An agent that
/// can't start does not fail the hub; it is `failed` and the rest run.
pub async fn run_hub(inputs: HubInputs) -> Result<HubExit, FatalError> {
    let hub_dir = residuum_config::paths::hub_dir(&inputs.root);
    let (hub_cfg, fallback_problem) = load_hub_with_fallback(&hub_dir)?;
    let gateway = bind_gateway(&hub_cfg.gateway).await?;
    let a2a_listener = bind_a2a(&hub_cfg).await;
    let runtime =
        HubRuntime::start(inputs, hub_cfg, fallback_problem, gateway, a2a_listener).await?;
    Ok(Box::pin(runtime.run(true)).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_workbench_sdk_reports_the_given_build() {
        let build = BuildInfo {
            version: "1.2.3",
            commit: None,
        };
        let sdk = workbench_sdk(build);
        assert_eq!(sdk.version, "1.2.3");
        assert_eq!(sdk.features, crate::features::FEATURES);
    }

    #[test]
    fn an_a2a_bind_failure_is_worded_by_its_io_cause() {
        let port: u16 = 8475;
        let in_use = anyhow::Error::from(std::io::Error::from(std::io::ErrorKind::AddrInUse))
            .context(format!(
                "failed to bind the A2A listener on 127.0.0.1:{port}"
            ));
        assert_eq!(
            a2a_failure_message(port, &in_use),
            format!(
                "The agent-to-agent listener couldn't start on port {port}: the port is already in use. Choose another port in settings or stop the program using it."
            )
        );

        let denied =
            anyhow::Error::from(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
                .context(format!(
                    "failed to bind the A2A listener on 127.0.0.1:{port}"
                ));
        assert_eq!(
            a2a_failure_message(port, &denied),
            format!(
                "The agent-to-agent listener couldn't start on port {port}: the system doesn't allow residuum to use that port. Choose another port in settings."
            )
        );

        let other = anyhow::anyhow!("a2a listener failed");
        assert_eq!(
            a2a_failure_message(port, &other),
            format!(
                "The agent-to-agent listener couldn't start on port {port}: something went wrong while starting it. Check the logs for details."
            )
        );
    }

    #[test]
    fn hub_fallback_uses_its_own_saved_copy_when_the_live_file_is_broken() {
        let dir = tempfile::tempdir().unwrap();
        let hub_dir = dir.path().join("hub");
        std::fs::create_dir_all(&hub_dir).unwrap();
        std::fs::write(hub_dir.join("config.toml"), "timezone = \"UTC\"\n").unwrap();
        last_known_good::hub::save(&hub_dir);
        std::fs::write(hub_dir.join("config.toml"), "not valid toml [[[").unwrap();

        let (hub, problem) = load_hub_with_fallback(&hub_dir).unwrap();

        assert_eq!(hub.timezone, chrono_tz::UTC);
        assert!(
            problem.is_some(),
            "a fallback should describe what was wrong with the live file"
        );
    }

    #[test]
    fn hub_fallback_without_a_saved_copy_reports_the_live_error() {
        let dir = tempfile::tempdir().unwrap();
        let hub_dir = dir.path().join("hub");
        std::fs::create_dir_all(&hub_dir).unwrap();
        std::fs::write(hub_dir.join("config.toml"), "not valid toml [[[").unwrap();

        assert!(load_hub_with_fallback(&hub_dir).is_err());
    }

    #[test]
    fn hub_fallback_starts_without_a_timezone_when_no_agent_exists() {
        let dir = tempfile::tempdir().unwrap();
        let hub_dir = dir.path().join("hub");
        std::fs::create_dir_all(&hub_dir).unwrap();
        std::fs::write(hub_dir.join("config.toml"), "# timezone = \"UTC\"\n").unwrap();

        let (hub, problem) = load_hub_with_fallback(&hub_dir).unwrap();

        assert!(problem.is_none());
        assert_ne!(hub.timezone.name(), "");
    }
}
