//! The hub process: everything that is one-per-process rather than
//! one-per-agent.
//!
//! [`run_hub`] starts the HTTP server, the relay tunnel, the A2A listener,
//! the workbench server, tracing, and the updater, builds the services every
//! agent shares (see [`HubServices`]), and hands the agents to the
//! [`AgentHost`]. It runs until the process is told to stop, then stops every
//! agent gracefully.

use std::path::Path;
use std::sync::Arc;

use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::Duration;

use crate::config::{Config, HubConfig};
use crate::gateway::last_known_good;
use crate::gateway::types::{GatewayExit, ReloadReceiver, ReloadSignal, TermSignal};
use crate::inference::EmbeddingProvider;
use crate::tunnel::TunnelStatus;
use crate::util::FatalError;

use super::directory::AgentDirectory;
use super::host::AgentHost;
use super::http::{HubHttpState, hub_router};
use super::relay_agents::RelayAgents;
use super::services::{HubControl, HubServices};
use super::team_embedding::EmbeddingSource;
use super::types::{HubEvent, NoticeLevel};

/// How often the hub checks for a newer version.
const UPDATE_CHECK_INTERVAL: Duration = Duration::from_hours(6);

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

/// Load the hub config, falling back to its last-known-good copy if the live
/// file fails to load. Returns the hub config actually used and — only when
/// the fallback was used — a description of what was wrong with the live
/// file.
fn load_hub_with_fallback(hub_dir: &Path) -> Result<(HubConfig, Option<String>), FatalError> {
    match HubConfig::load_at(hub_dir) {
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
    let names = match crate::config::discover_agents(root) {
        Ok(names) => names,
        Err(e) => {
            tracing::warn!(error = %e, "couldn't scan for agents before start-up");
            return scan;
        }
    };
    for name in names {
        let agent_dir = crate::config::paths::agent_dir(root, &name);
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

/// Bind the HTTP server and serve `app` on it.
///
/// # Errors
/// Returns `FatalError::Gateway` if the address cannot be bound.
async fn spawn_http_server(
    gateway: &crate::config::GatewayConfig,
    app: axum::Router,
) -> Result<HttpServer, FatalError> {
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
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
    let handle = crate::util::spawn_in_span(async move {
        if let Err(e) = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                shutdown_rx.wait_for(|stop| *stop).await.ok();
            })
            .await
        {
            tracing::error!(error = %e, "gateway server error");
        }
    });
    Ok(HttpServer {
        handle,
        shutdown_tx,
    })
}

/// The handles a relay tunnel is started with.
struct TunnelInputs<'a> {
    workbench_port: Option<u16>,
    status_tx: &'a Arc<watch::Sender<TunnelStatus>>,
    relay_agents: &'a RelayAgents,
}

/// Start the relay tunnel for `cloud`. The tunnel forwards A2A requests to the
/// hub's A2A listener when it is enabled, and keeps the relay's agent list
/// current from `relay_agents`.
fn spawn_tunnel(
    hub: &HubConfig,
    cloud: &crate::config::CloudConfig,
    inputs: &TunnelInputs<'_>,
) -> TunnelTask {
    let cloud = cloud.clone();
    let workbench_port = inputs.workbench_port;
    let a2a_port = hub.a2a.enabled.then_some(hub.a2a.port);
    let agents_rx = inputs.relay_agents.subscribe();
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let status_tx = Arc::clone(inputs.status_tx);
    let handle = crate::util::spawn_monitored("tunnel", async move {
        crate::tunnel::start_tunnel(
            cloud,
            workbench_port,
            a2a_port,
            agents_rx,
            shutdown_rx,
            status_tx,
        )
        .await;
    });
    TunnelTask {
        handle,
        shutdown_tx,
    }
}

/// Start the hub's A2A listener over `host`, serving every agent under
/// `/agents/{name}/`.
fn spawn_a2a_listener(
    hub: &HubConfig,
    host: &Arc<AgentHost>,
    services: &HubServices,
) -> A2aListenerTask {
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let listener = crate::a2a::A2aListener::new(
        hub.gateway.bind.clone(),
        hub.a2a.port,
        Arc::clone(host) as Arc<dyn AgentDirectory>,
        Arc::clone(&services.a2a_keys),
        Arc::new(|| Some(Arc::<str>::from(crate::tunnel::tunnel_nonce()))),
        shutdown_rx,
    );
    let handle = crate::util::spawn_monitored("a2a", async move {
        if let Err(e) = listener.start().await {
            tracing::error!(error = %e, "a2a interface failed");
        }
    });
    A2aListenerTask {
        handle,
        shutdown_tx,
    }
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

/// The hub's process-level state.
struct HubRuntime {
    hub_dir: std::path::PathBuf,
    hub_cfg: HubConfig,
    services: HubServices,
    host: Arc<AgentHost>,
    app: axum::Router,
    server: HttpServer,
    /// Resolves when onboarding has written the first agent; `None` once the
    /// hub has agents (or from the start, if it began with some).
    setup_done_rx: Option<watch::Receiver<bool>>,
    tunnel: Option<TunnelTask>,
    tunnel_status_tx: Arc<watch::Sender<TunnelStatus>>,
    relay_agents: RelayAgents,
    a2a: Option<A2aListenerTask>,
    workbench_shutdown_tx: Option<watch::Sender<bool>>,
    reload_rx: ReloadReceiver,
    watcher: JoinHandle<()>,
    restart_rx: mpsc::Receiver<()>,
    shutdown_rx: mpsc::Receiver<()>,
    sigterm: TermSignal,
}

impl HubRuntime {
    /// Start the hub on the residuum root `root`: build the shared services
    /// and the host, bring up the servers, and start the `autostart` agents.
    async fn start(
        root: &Path,
        hub_cfg: HubConfig,
        fallback_problem: Option<String>,
    ) -> Result<Self, FatalError> {
        let hub_dir = hub_cfg.config_dir.clone();
        let (restart_tx, restart_rx) = mpsc::channel::<()>(1);
        let (shutdown_tx, shutdown_rx) = mpsc::channel::<()>(1);
        let (tunnel_status_tx, tunnel_status_rx) = watch::channel(TunnelStatus::Disconnected);
        let tunnel_status_tx = Arc::new(tunnel_status_tx);

        let scan = scan_agents(root, &hub_cfg);
        let (workbench_serving, workbench_shutdown_tx) =
            start_workbench_listener(root, &hub_cfg, &scan.teams_ports).await;
        let services = HubServices::open(
            root,
            &hub_cfg,
            tunnel_status_rx,
            HubControl {
                update_status: crate::update::SharedUpdateStatus::default(),
                restart_tx,
                shutdown_tx,
            },
            workbench_serving,
            scan.team_embedding.as_ref().and_then(build_team_embedding),
        )
        .await?;

        let host = AgentHost::new(services.clone(), hub_cfg.clone());
        host.note_team_embedding(scan.team_embedding).await;
        let agents = host.discover()?;
        tracing::info!(agents = agents.len(), names = %agents.join(", "), "found agents");
        publish_startup_notices(&host, &hub_cfg, fallback_problem.as_deref());

        let (reload_tx, reload_rx) = tokio::sync::mpsc::unbounded_channel();
        // With no agents the web app runs onboarding, whose last step signals
        // this once the first agent is on disk.
        let setup_done = agents.is_empty().then(|| Arc::new(watch::channel(false).0));
        let setup_done_rx = setup_done.as_ref().map(|tx| tx.subscribe());
        let app = build_app(&host, &services, reload_tx.clone(), setup_done)?;
        let server = spawn_http_server(&hub_cfg.gateway, app.clone()).await?;
        let a2a = hub_cfg
            .a2a
            .enabled
            .then(|| spawn_a2a_listener(&hub_cfg, &host, &services));
        let relay_agents = RelayAgents::spawn(
            Arc::clone(&host) as Arc<dyn AgentDirectory>,
            hub_cfg.a2a.enabled,
        );
        let tunnel = hub_cfg.cloud.as_ref().map(|cloud| {
            spawn_tunnel(
                &hub_cfg,
                cloud,
                &TunnelInputs {
                    workbench_port: services.workbench_serving.port(),
                    status_tx: &tunnel_status_tx,
                    relay_agents: &relay_agents,
                },
            )
        });
        let watcher = crate::gateway::watcher::spawn_hub_config_watcher(
            hub_dir.join("config.toml"),
            reload_tx,
        );
        #[cfg(unix)]
        let sigterm = TermSignal::new().map_err(|e| {
            FatalError::Gateway(format!("failed to register termination handler: {e}"))
        })?;
        #[cfg(not(unix))]
        let sigterm = TermSignal::new();

        host.start_autostart().await;

        // Reaching this point means the servers are bound and every
        // autostart agent has either started or been recorded as failed.
        crate::daemon::write_ready_file(&hub_dir);
        if fallback_problem.is_none() {
            last_known_good::hub::save(&hub_dir);
        }

        Ok(Self {
            hub_dir,
            hub_cfg,
            services,
            host,
            app,
            server,
            setup_done_rx,
            tunnel,
            tunnel_status_tx,
            relay_agents,
            a2a,
            workbench_shutdown_tx,
            reload_rx,
            watcher,
            restart_rx,
            shutdown_rx,
            sigterm,
        })
    }

    /// Run until told to stop, then stop every agent and the servers.
    /// `check_updates` turns the periodic version check on.
    async fn run(mut self, check_updates: bool) -> GatewayExit {
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
                    break GatewayExit::Shutdown;
                }
                _ = self.shutdown_rx.recv() => {
                    tracing::info!("shutdown requested through the HTTP API");
                    break GatewayExit::Shutdown;
                }
                _ = self.restart_rx.recv() => {
                    tracing::info!("restart requested, shutting down for re-exec");
                    break GatewayExit::Restart;
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
                crate::tracing_service::client_context::gather_for_hub(),
            )
            .await;
        self.tunnel_status_tx.send(TunnelStatus::Disconnected).ok();
        if let Some(cloud) = &self.hub_cfg.cloud {
            self.tunnel = Some(spawn_tunnel(
                &self.hub_cfg,
                cloud,
                &TunnelInputs {
                    workbench_port: self.services.workbench_serving.port(),
                    status_tx: &self.tunnel_status_tx,
                    relay_agents: &self.relay_agents,
                },
            ));
            tracing::info!("tunnel respawned after unexpected exit");
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
                let message =
                    format!("hub config reload failed (keeping current hub config): {err}");
                self.host.notice(NoticeLevel::Warn, message.clone(), None);
                self.host.publish(HubEvent::HubConfigReloaded {
                    ok: false,
                    changed: false,
                    message: Some(message),
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
        if old.background.max_concurrent != new_hub.background.max_concurrent {
            changed.push("background limits");
            self.host.notice(
                NoticeLevel::Info,
                "background.max_concurrent changed in hub/config.toml — this takes effect on the next restart, not immediately.".to_string(),
                None,
            );
        } else if old.background != new_hub.background {
            changed.push("background limits");
        }

        self.host.hub_config_changed(new_hub);
        last_known_good::hub::save(&self.hub_dir);
        let summary = changed.join(", ");
        tracing::info!(changes = %summary, "hub configuration reloaded successfully");
        let message = format!("hub configuration reloaded: {summary}");
        self.host.notice(NoticeLevel::Info, message.clone(), None);
        self.host.publish(HubEvent::HubConfigReloaded {
            ok: true,
            changed: true,
            message: Some(message),
        });
    }

    /// Serve the hub's HTTP app on the new address, then retire the old
    /// server. A bind failure keeps the current server and tells the user.
    async fn rebind_http(&mut self, new_hub: &HubConfig) {
        match spawn_http_server(&new_hub.gateway, self.app.clone()).await {
            Ok(server) => {
                let old = std::mem::replace(&mut self.server, server);
                old.shutdown_tx.send(true).ok();
                tracing::info!(addr = %new_hub.gateway.addr(), "gateway rebound to new address");
            }
            Err(e) => {
                tracing::warn!(error = %e, "failed to bind to the new gateway address, keeping the current server");
                self.host.notice(
                    NoticeLevel::Warn,
                    format!(
                        "gateway rebind failed ({}): {e} — keeping the current server",
                        new_hub.gateway.addr()
                    ),
                    None,
                );
            }
        }
    }

    /// Push a changed `[tracing]` section into the tracing service and the
    /// global log filter.
    async fn apply_tracing(&self, new_hub: &HubConfig) {
        self.services
            .tracing_service
            .update_config(new_hub.tracing.clone())
            .await;
        if let Some(handle) = crate::util::tracing_init::global_filter_handle()
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
            self.tunnel = Some(spawn_tunnel(
                new_hub,
                cloud,
                &TunnelInputs {
                    workbench_port: self.services.workbench_serving.port(),
                    status_tx: &self.tunnel_status_tx,
                    relay_agents: &self.relay_agents,
                },
            ));
            tracing::info!("tunnel restarted with new config");
        } else {
            tracing::info!("cloud tunnel removed from config");
        }
    }

    /// Stop the A2A listener (if running) and start one for the new config.
    async fn restart_a2a_listener(&mut self, new_hub: &HubConfig) {
        if let Some(a2a) = self.a2a.take() {
            stop_task("a2a", &a2a.shutdown_tx, a2a.handle).await;
        }
        if new_hub.a2a.enabled {
            self.a2a = Some(spawn_a2a_listener(new_hub, &self.host, &self.services));
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
/// `services`.
///
/// # Errors
/// Returns `FatalError::Gateway` if the hub-level checkpoint repositories
/// can't be opened.
pub(super) fn build_app(
    host: &Arc<AgentHost>,
    services: &HubServices,
    reload_tx: crate::gateway::types::ReloadSender,
    setup_done: Option<Arc<watch::Sender<bool>>>,
) -> Result<axum::Router, FatalError> {
    let checkpoints = crate::checkpoints::CheckpointEngine::hub_scoped(
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
        secret_lock: Arc::clone(&services.secret_lock),
        checkpoints: Arc::new(checkpoints),
        tunnel_status_rx: services.tunnel_status_rx.clone(),
        update_status: Arc::clone(&services.control.update_status),
        restart_tx: services.control.restart_tx.clone(),
        shutdown_tx: services.control.shutdown_tx.clone(),
        tracing_service: Arc::clone(&services.tracing_service),
        client_context: Arc::new(crate::tracing_service::client_context::gather_for_hub()),
        active_subagents: Arc::new(move || subagents_host.active_subagents()),
        workbench_serving: services.workbench_serving.clone(),
        team: services.team.clone(),
        team_bus: services.team_feed.bus.clone(),
        team_watch_health: services.team_feed.health.clone(),
        started_at: std::time::Instant::now(),
        push: Arc::clone(&services.push),
        boot_id: uuid::Uuid::new_v4().to_string(),
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
async fn start_workbench_listener(
    root: &Path,
    hub: &HubConfig,
    teams_ports: &[u16],
) -> (
    crate::workbench::server::WorkbenchServing,
    Option<watch::Sender<bool>>,
) {
    let mut reserved = vec![
        crate::config::DEFAULT_TEAMS_PORT,
        hub.a2a.port,
        crate::config::DEFAULT_A2A_PORT,
    ];
    reserved.extend_from_slice(teams_ports);
    let workbench_dir =
        crate::config::paths::TeamPaths::new(crate::config::paths::team_dir(root)).workbench_dir();
    crate::workbench::server::start(
        &hub.gateway.bind,
        hub.gateway.port,
        &reserved,
        workbench_dir,
    )
    .await
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
    for notice in crate::config::resolve::removed_agent_env_override_notices() {
        tracing::warn!(%notice, "removed environment override is still set");
        host.notice(NoticeLevel::Warn, notice, None);
    }
}

/// Spawn a fire-and-forget update check task.
fn spawn_update_check(status: &crate::update::SharedUpdateStatus) {
    let status = Arc::clone(status);
    crate::util::spawn_monitored("update-check", async move {
        crate::update::check_for_update(&status).await;
    });
}

/// Run the hub on the residuum root `root` until it is told to stop.
///
/// Loads the hub config (falling back to its last-known-good copy), starts
/// the servers and the shared services, and starts every agent whose
/// `autostart` is on. Returns [`GatewayExit::Restart`] when the binary was
/// updated and the process should relaunch.
///
/// # Errors
///
/// Returns `FatalError` if the hub config can't be loaded and has no working
/// last-known-good copy, or a hub-level service (the HTTP server, the team
/// wiki index, the checkpoint repositories) cannot start. An agent that
/// can't start does not fail the hub; it is `failed` and the rest run.
pub async fn run_hub(root: &Path) -> Result<GatewayExit, FatalError> {
    let hub_dir = crate::config::paths::hub_dir(root);
    let (hub_cfg, fallback_problem) = load_hub_with_fallback(&hub_dir)?;
    let runtime = HubRuntime::start(root, hub_cfg, fallback_problem).await?;
    Ok(Box::pin(runtime.run(true)).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::test_support::{free_port, mount_reply, write_agent};
    use wiremock::MockServer;

    /// What the hub said about one config reload.
    struct Reload {
        ok: bool,
        changed: bool,
        message: Option<String>,
        /// The notices sent before the `hub_config_reloaded` frame.
        notices: Vec<String>,
    }

    /// A hub started on temp directories with two agents, running on free
    /// ports, with the model server all its agents talk to.
    struct RunningHub {
        root: tempfile::TempDir,
        gateway_port: u16,
        a2a_port: u16,
        events: tokio::sync::broadcast::Receiver<HubEvent>,
        host: Arc<AgentHost>,
        exit: JoinHandle<GatewayExit>,
        http: reqwest::Client,
        model: MockServer,
    }

    impl RunningHub {
        async fn start() -> Self {
            Self::start_with(&["atlas", "scout"]).await
        }

        /// A hub over the agents `names`, which may be none.
        async fn start_with(names: &[&str]) -> Self {
            let root = tempfile::tempdir().unwrap();
            let (gateway_port, a2a_port) = (free_port(), free_port());
            let hub_dir = root.path().join("hub");
            std::fs::create_dir_all(&hub_dir).unwrap();
            std::fs::write(
                hub_dir.join("config.toml"),
                format!(
                    "timezone = \"UTC\"\n[gateway]\nport = {gateway_port}\n[a2a]\nport = {a2a_port}\n"
                ),
            )
            .unwrap();
            let model = MockServer::start().await;
            mount_reply(&model, "hello", Duration::ZERO).await;
            for name in names {
                write_agent(root.path(), name, &model.uri());
            }
            let runtime =
                HubRuntime::start(root.path(), HubConfig::load_at(&hub_dir).unwrap(), None)
                    .await
                    .unwrap();
            let events = runtime.host.subscribe();
            let host = Arc::clone(&runtime.host);
            Self {
                root,
                gateway_port,
                a2a_port,
                events,
                host,
                exit: crate::util::spawn_in_span(runtime.run(false)),
                http: reqwest::Client::new(),
                model,
            }
        }

        async fn status_of(&self, port: u16, path: &str) -> Option<u16> {
            self.http
                .get(format!("http://127.0.0.1:{port}{path}"))
                .send()
                .await
                .ok()
                .map(|response| response.status().as_u16())
        }

        async fn eventually_status(&self, port: u16, path: &str, expected: Option<u16>) {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
            while self.status_of(port, path).await != expected {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "timed out waiting for {path} on port {port} to answer {expected:?}"
                );
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }

        fn hub_config_path(&self) -> std::path::PathBuf {
            self.root.path().join("hub").join("config.toml")
        }

        /// Wait for the hub to announce a finished config reload, and return
        /// its frame with the notices that came before it.
        async fn next_reload(&mut self) -> Reload {
            tokio::time::timeout(Duration::from_secs(20), async {
                let mut notices = Vec::new();
                loop {
                    match self.events.recv().await.unwrap() {
                        HubEvent::Notice { message, .. } => notices.push(message),
                        HubEvent::HubConfigReloaded {
                            ok,
                            changed,
                            message,
                        } => {
                            return Reload {
                                ok,
                                changed,
                                message,
                                notices,
                            };
                        }
                        HubEvent::AgentState { .. }
                        | HubEvent::AgentStopping { .. }
                        | HubEvent::AgentCreated { .. }
                        | HubEvent::AgentRestored { .. }
                        | HubEvent::AgentDeleted { .. }
                        | HubEvent::AgentActivity { .. } => {}
                    }
                }
            })
            .await
            .expect("the hub announces the reload")
        }

        async fn shut_down(self) {
            let response = self
                .http
                .post(format!(
                    "http://127.0.0.1:{}/api/hub/shutdown",
                    self.gateway_port
                ))
                .send()
                .await
                .unwrap();
            assert!(response.status().is_success());
            let exit = tokio::time::timeout(Duration::from_secs(60), self.exit)
                .await
                .expect("the hub stops after a shutdown request")
                .unwrap();
            assert!(matches!(exit, GatewayExit::Shutdown));
        }
    }

    #[tokio::test]
    async fn the_hub_serves_its_agents_on_one_port_and_stops_them_on_shutdown() {
        let hub = RunningHub::start().await;

        for name in ["atlas", "scout"] {
            hub.eventually_status(
                hub.gateway_port,
                &format!("/api/agents/{name}/status"),
                Some(200),
            )
            .await;
            hub.eventually_status(
                hub.a2a_port,
                &format!("/agents/{name}/.well-known/agent-card.json"),
                Some(200),
            )
            .await;
        }
        assert!(
            hub.root.path().join("hub").join("residuum.ready").is_file(),
            "the readiness marker is written once the agents are up"
        );
        let gateway_port = hub.gateway_port;
        let http = hub.http.clone();

        hub.shut_down().await;

        assert!(
            http.get(format!(
                "http://127.0.0.1:{gateway_port}/api/agents/scout/status"
            ))
            .send()
            .await
            .is_err(),
            "the server is gone"
        );
    }

    #[tokio::test]
    async fn changing_the_gateway_port_rebinds_the_server_and_keeps_the_agents_running() {
        let hub = RunningHub::start().await;
        hub.eventually_status(hub.gateway_port, "/api/agents/scout/status", Some(200))
            .await;
        let new_port = free_port();

        std::fs::write(
            hub.hub_config_path(),
            format!(
                "timezone = \"UTC\"\n[gateway]\nport = {new_port}\n[a2a]\nport = {}\n",
                hub.a2a_port
            ),
        )
        .unwrap();

        hub.eventually_status(new_port, "/api/agents/scout/status", Some(200))
            .await;
        hub.eventually_status(new_port, "/api/agents/atlas/status", Some(200))
            .await;
        hub.eventually_status(hub.gateway_port, "/api/agents/scout/status", None)
            .await;
        let mut hub = hub;
        hub.gateway_port = new_port;
        hub.shut_down().await;
    }

    #[tokio::test]
    async fn a_broken_hub_config_reload_keeps_the_hub_running_and_says_so() {
        let mut hub = RunningHub::start().await;
        hub.eventually_status(hub.gateway_port, "/api/agents/scout/status", Some(200))
            .await;

        std::fs::write(hub.hub_config_path(), "not valid toml [[[").unwrap();

        let reload = hub.next_reload().await;
        assert!(
            !reload.ok && !reload.changed,
            "a failed reload changes nothing"
        );
        let message = reload.message.expect("a failed reload says why");
        assert!(message.starts_with("hub config reload failed"), "{message}");
        assert!(message.contains("keeping current hub config"), "{message}");
        assert_eq!(
            reload.notices,
            [message],
            "the notice says what the frame says"
        );
        hub.eventually_status(hub.gateway_port, "/api/agents/scout/status", Some(200))
            .await;
        hub.shut_down().await;
    }

    #[tokio::test]
    async fn a_hub_config_reload_that_changes_a_setting_says_what_changed() {
        let mut hub = RunningHub::start().await;
        hub.eventually_status(hub.gateway_port, "/api/agents/scout/status", Some(200))
            .await;

        let config = std::fs::read_to_string(hub.hub_config_path()).unwrap();
        std::fs::write(
            hub.hub_config_path(),
            config.replace("timezone = \"UTC\"", "timezone = \"America/Chicago\""),
        )
        .unwrap();

        let reload = hub.next_reload().await;
        assert!(reload.ok && reload.changed);
        assert_eq!(
            reload.message.as_deref(),
            Some("hub configuration reloaded: timezone")
        );
        assert_eq!(reload.notices, ["hub configuration reloaded: timezone"]);
        hub.shut_down().await;
    }

    #[tokio::test]
    async fn a_hub_config_reload_that_finds_nothing_new_says_so_without_a_notice() {
        let mut hub = RunningHub::start().await;
        hub.eventually_status(hub.gateway_port, "/api/agents/scout/status", Some(200))
            .await;

        // A comment changes the file and not the settings in it.
        let config = std::fs::read_to_string(hub.hub_config_path()).unwrap();
        std::fs::write(hub.hub_config_path(), format!("# a note to self\n{config}")).unwrap();

        let reload = hub.next_reload().await;
        assert!(reload.ok, "the file loaded");
        assert!(!reload.changed, "and it matches what was running");
        assert_eq!(reload.message, None);
        assert!(reload.notices.is_empty(), "{:?}", reload.notices);
        hub.shut_down().await;
    }

    /// The body onboarding posts to create the first agent `name`, talking to
    /// the hub's mock model.
    fn setup_body(hub: &RunningHub, name: &str) -> serde_json::Value {
        serde_json::json!({
            "hub_config": format!(
                "timezone = \"UTC\"\n[gateway]\nport = {}\n[a2a]\nport = {}\n",
                hub.gateway_port, hub.a2a_port
            ),
            "agent_name": name,
            "user_name": "Sam",
            "config": "",
            "providers": format!(
                "[providers]\nmock = {{ type = \"openai\", api_key = \"test-key\", url = \"{}\" }}\n\n[models]\nmain = \"mock/test-model\"\n",
                hub.model.uri()
            ),
        })
    }

    #[tokio::test]
    async fn onboarding_a_hub_without_agents_starts_the_first_agent_without_a_restart() {
        let hub = RunningHub::start_with(&[]).await;
        hub.eventually_status(hub.gateway_port, "/api/hub/agents", Some(200))
            .await;
        assert_eq!(
            hub.status_of(hub.gateway_port, "/api/agents/scout/status")
                .await,
            Some(404),
            "no agent exists before onboarding"
        );
        let complete_setup = format!(
            "http://127.0.0.1:{}/api/hub/config/complete-setup",
            hub.gateway_port
        );

        let response = hub
            .http
            .post(&complete_setup)
            .json(&setup_body(&hub, "scout"))
            .send()
            .await
            .unwrap();

        assert!(response.status().is_success(), "{response:?}");
        hub.eventually_status(hub.gateway_port, "/api/agents/scout/status", Some(200))
            .await;
        hub.eventually_status(
            hub.a2a_port,
            "/agents/scout/.well-known/agent-card.json",
            Some(200),
        )
        .await;
        let again = hub
            .http
            .post(&complete_setup)
            .json(&setup_body(&hub, "atlas"))
            .send()
            .await
            .unwrap();
        assert_eq!(
            again.status().as_u16(),
            409,
            "onboarding only creates the first agent"
        );
        hub.shut_down().await;
    }

    #[tokio::test]
    async fn editing_a_team_identity_file_reloads_every_running_agent() {
        let hub = RunningHub::start().await;
        for name in ["atlas", "scout"] {
            hub.eventually_status(
                hub.gateway_port,
                &format!("/api/agents/{name}/status"),
                Some(200),
            )
            .await;
        }
        let reloads_before =
            ["atlas", "scout"].map(|name| hub.host.reloads_finished(name).unwrap());

        let response = hub
            .http
            .put(format!(
                "http://127.0.0.1:{}/api/team/workspace/file",
                hub.gateway_port
            ))
            .json(
                &serde_json::json!({ "path": "USER.md", "content": "Sam prefers short replies." }),
            )
            .send()
            .await
            .unwrap();

        assert!(response.status().is_success(), "{response:?}");
        for (name, before) in ["atlas", "scout"].into_iter().zip(reloads_before) {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
            while hub.host.reloads_finished(name).unwrap() <= before {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "{name} never reloaded after the team USER.md changed"
                );
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
        hub.shut_down().await;
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
}
