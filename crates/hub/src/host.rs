use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock, Weak};
use std::time::Duration;

use async_trait::async_trait;
use axum::Router;
use chrono::Utc;
use serde::Deserialize;
use tokio::sync::{broadcast, watch};
use tokio::task::AbortHandle;
use tracing::Instrument;

use residuum_bus::control::ReloadSignal;
use residuum_bus::session::SessionInfo;
use residuum_config::paths::TeamPaths;
use residuum_config::{Config, HubConfig, WorkspaceLayout};
use residuum_core::util::FatalError;
use residuum_inference::{PriceTable, UsageRecorder};

use super::activity::ActivityTracker;
use super::agent_watch::{AgentChange, AgentChangeFeed, AgentChangeKind, AgentWatcher};
use super::directory::{AgentDirectory, AgentFiles};
use super::launch::{
    AgentCleanup, AgentControl, AgentExit, AgentLauncher, AgentStartInputs, StartedAgent,
    agent_span,
};
use super::services::HubServices;
use super::team_embedding::EmbeddingSource;
use super::types::{
    AgentActivity, AgentPatch, DeletedAgent, HubEvent, HubNoticeKind, NoticeLevel,
    RestoreAgentRequest,
};
use residuum_background::team::{
    A2aVisibility, Actor, AgentErrorKind, AgentLastError, AgentState, AgentSummary,
    CreateAgentRequest, DeleteOutcome, LifecycleError, TeamDirectory,
};
use residuum_workspace::team_files::TeamWriter;

const HUB_EVENT_CAPACITY: usize = 256;

const STOP_TIMEOUT: Duration = Duration::from_secs(150);

const ABORT_SETTLE: Duration = Duration::from_secs(10);

const PATCH_RELOAD_TIMEOUT: Duration = Duration::from_secs(30);

const GRACEFUL_RESTART_TIMEOUT: Duration = Duration::from_secs(300);

const SHUTTING_DOWN: &str = "Residuum is shutting down";

fn teams_section_is_configured(doc: &toml_edit::DocumentMut) -> bool {
    let Some(teams) = doc.get("teams") else {
        return false;
    };
    ["app_id", "tenant_id", "app_password"].iter().all(|key| {
        teams
            .get(key)
            .and_then(toml_edit::Item::as_str)
            .is_some_and(|value| !value.trim().is_empty())
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AgentMeta {
    autostart: bool,
    a2a_visibility: A2aVisibility,
    display_name: String,
    teams_configured: bool,
}

impl AgentMeta {
    fn unverified(name: &str) -> Self {
        Self {
            autostart: true,
            a2a_visibility: A2aVisibility::Private,
            display_name: name.to_string(),
            teams_configured: false,
        }
    }

    fn read(agent_dir: &Path, name: &str, had_meta: bool) -> Result<Self, String> {
        let path = agent_dir.join("config").join("config.toml");
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("couldn't read {}: {e}", path.display()))?;
        if text.trim().is_empty() {
            if had_meta {
                return Err(format!("{} is empty", path.display()));
            }
            return Ok(Self {
                autostart: true,
                a2a_visibility: A2aVisibility::Public,
                display_name: name.to_string(),
                teams_configured: false,
            });
        }
        let doc = text
            .parse::<toml_edit::DocumentMut>()
            .map_err(|e| format!("couldn't parse {}: {e}", path.display()))?;
        let autostart = doc
            .get("autostart")
            .and_then(toml_edit::Item::as_bool)
            .unwrap_or(true);
        let a2a_visibility = match doc
            .get("a2a")
            .and_then(|a2a| a2a.get("visibility"))
            .and_then(toml_edit::Item::as_str)
            .map(str::trim)
        {
            None | Some("" | "public") => A2aVisibility::Public,
            Some("private") => A2aVisibility::Private,
            Some(other) => {
                tracing::warn!(value = other, path = %path.display(), "an agent's [a2a] visibility is not \"public\" or \"private\"; treating it as private");
                A2aVisibility::Private
            }
        };
        let display_name = match doc.get("display_name").and_then(toml_edit::Item::as_str) {
            Some(raw) => match residuum_config::canonicalize_display_name(raw) {
                Ok(cleaned) => cleaned,
                Err(problem) => {
                    tracing::warn!(agent = name, problem, path = %path.display(), "display_name in config.toml can't be shown; using the folder name");
                    name.to_string()
                }
            },
            None => name.to_string(),
        };
        Ok(Self {
            autostart,
            a2a_visibility,
            display_name,
            teams_configured: teams_section_is_configured(&doc),
        })
    }
}

#[derive(Deserialize)]
struct RoleFrontmatter {
    description: Option<String>,
}

fn role_line(team: &TeamPaths, name: &str) -> Option<String> {
    let page = team.agent_role_page(name);
    let text = std::fs::read_to_string(&page).ok()?;
    match residuum_core::util::parse_frontmatter_md::<RoleFrontmatter>(&text, "agent role page") {
        Ok((frontmatter, _body)) => frontmatter
            .description
            .map(|description| description.trim().to_string())
            .filter(|description| !description.is_empty()),
        Err(e) => {
            tracing::debug!(error = %e, page = %page.display(), "an agent's role page has no usable frontmatter");
            None
        }
    }
}

pub struct RunningAgent {
    pub(crate) control: AgentControl,
    pub(crate) stop_requested: Arc<AtomicBool>,
    forced: Arc<AtomicBool>,
    done: watch::Receiver<bool>,
    abort: AbortHandle,
}

impl RunningAgent {
    #[must_use]
    pub fn control(&self) -> &AgentControl {
        &self.control
    }

    #[must_use]
    pub fn stop_requested(&self) -> &Arc<AtomicBool> {
        &self.stop_requested
    }
}

pub struct SlotState {
    state: AgentState,
    last_error: Option<AgentLastError>,
    pub(crate) running: Option<RunningAgent>,
    generation: u64,
    published_meta: Option<AgentMeta>,
    meta_unreadable: bool,
}

impl SlotState {
    #[must_use]
    pub fn running(&self) -> Option<&RunningAgent> {
        self.running.as_ref()
    }
}

pub struct AgentSlot {
    name: String,
    dir: PathBuf,
    op_lock: tokio::sync::Mutex<()>,
    removed: AtomicBool,
    state: Mutex<SlotState>,
    activity: Arc<ActivityTracker>,
}

impl AgentSlot {
    pub fn lock(&self) -> std::sync::MutexGuard<'_, SlotState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn is_removed(&self) -> bool {
        self.removed.load(Ordering::SeqCst)
    }

    fn ensure_present(&self) -> Result<(), LifecycleError> {
        if self.is_removed() {
            return Err(LifecycleError::NotFound(self.name.clone()));
        }
        Ok(())
    }

    fn reload_meta(&self) -> (AgentMeta, bool) {
        let had_meta = self.lock().published_meta.is_some();
        let read = AgentMeta::read(&self.dir, &self.name, had_meta);
        let mut guard = self.lock();
        match read {
            Ok(meta) => {
                let changed = guard.published_meta.as_ref() != Some(&meta);
                guard.published_meta = Some(meta.clone());
                guard.meta_unreadable = false;
                (meta, changed)
            }
            Err(reason) => {
                if !guard.meta_unreadable {
                    guard.meta_unreadable = true;
                    let holding = if guard.published_meta.is_some() {
                        "keeping the settings from the last config that loaded"
                    } else {
                        "no config has loaded yet, so it is treated as private"
                    };
                    tracing::warn!(agent = %self.name, %reason, "an agent's config file can't be used for its summary; {holding}");
                }
                (
                    guard
                        .published_meta
                        .clone()
                        .unwrap_or_else(|| AgentMeta::unverified(&self.name)),
                    false,
                )
            }
        }
    }

    fn current_meta(&self) -> AgentMeta {
        {
            let guard = self.lock();
            if guard.running.is_some()
                && let Some(meta) = guard.published_meta.clone()
            {
                return meta;
            }
        }
        self.reload_meta().0
    }
}

pub struct AgentHost {
    me: Weak<Self>,
    services: HubServices,
    launcher: Arc<dyn AgentLauncher>,
    hub_cfg: RwLock<HubConfig>,
    slots: RwLock<BTreeMap<String, Arc<AgentSlot>>>,
    events: broadcast::Sender<HubEvent>,
    agent_changes: Arc<AgentChangeFeed>,
    creation_lock: tokio::sync::Mutex<()>,
    stopping: AtomicBool,
    teams_ports: Mutex<BTreeMap<u16, String>>,
    teams_ports_tx: watch::Sender<BTreeMap<String, u16>>,
    team_embedding: tokio::sync::Mutex<Option<EmbeddingSource>>,
    graceful_restart_timeout: Mutex<Duration>,
}

impl AgentHost {
    pub fn new(
        services: HubServices,
        hub_cfg: HubConfig,
        launcher: Arc<dyn AgentLauncher>,
    ) -> Arc<Self> {
        let (events, _first_subscriber) = broadcast::channel(HUB_EVENT_CAPACITY);
        let (teams_ports_tx, _) = watch::channel(BTreeMap::new());
        Arc::new_cyclic(|me| {
            services
                .directory
                .bind(Weak::clone(me) as Weak<dyn TeamDirectory>);
            services.mailboxes.bind(
                Weak::clone(me) as Weak<dyn residuum_workspace::inbox::mail::MailboxDirectory>
            );
            Self {
                me: Weak::clone(me),
                services,
                launcher,
                hub_cfg: RwLock::new(hub_cfg),
                slots: RwLock::new(BTreeMap::new()),
                events,
                agent_changes: AgentChangeFeed::new(),
                creation_lock: tokio::sync::Mutex::new(()),
                stopping: AtomicBool::new(false),
                teams_ports: Mutex::new(BTreeMap::new()),
                teams_ports_tx,
                team_embedding: tokio::sync::Mutex::new(None),
                graceful_restart_timeout: Mutex::new(GRACEFUL_RESTART_TIMEOUT),
            }
        })
    }

    fn graceful_restart_timeout(&self) -> Duration {
        *self
            .graceful_restart_timeout
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn set_graceful_restart_timeout(&self, timeout: Duration) {
        *self
            .graceful_restart_timeout
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = timeout;
    }

    fn slots(&self) -> Vec<Arc<AgentSlot>> {
        self.slots
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .cloned()
            .collect()
    }

    pub fn slot(&self, name: &str) -> Result<Arc<AgentSlot>, LifecycleError> {
        let slots = self.slots.read().unwrap_or_else(PoisonError::into_inner);
        if let Some(slot) = slots.get(name) {
            return Ok(Arc::clone(slot));
        }
        let Ok(canonical) = residuum_config::canonicalize_display_name(name) else {
            return Err(LifecycleError::NotFound(name.to_string()));
        };
        let key = residuum_config::display_name_key(&canonical);
        let mut found = None;
        for slot in slots.values() {
            if residuum_config::display_name_key(&slot.current_meta().display_name) != key {
                continue;
            }
            if found.is_some() {
                tracing::warn!(
                    name,
                    "more than one agent is called this; address it by its folder name"
                );
                return Err(LifecycleError::NotFound(name.to_string()));
            }
            found = Some(Arc::clone(slot));
        }
        found.ok_or_else(|| LifecycleError::NotFound(name.to_string()))
    }

    pub fn team_paths(&self) -> TeamPaths {
        TeamPaths::new(residuum_config::paths::team_dir(&self.services.root))
    }

    pub fn hub_config(&self) -> HubConfig {
        self.hub_cfg
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn publish(&self, event: HubEvent) {
        self.events.send(event).ok();
    }

    #[must_use]
    pub fn agent_changes(&self) -> &Arc<AgentChangeFeed> {
        &self.agent_changes
    }

    pub fn notice(&self, level: NoticeLevel, message: String, agent: Option<String>) {
        self.publish(HubEvent::notice(level, message, agent));
    }

    pub fn notice_of_kind(&self, level: NoticeLevel, kind: HubNoticeKind, message: String) {
        self.publish(HubEvent::Notice {
            level,
            message,
            agent: None,
            kind: Some(kind),
        });
    }

    pub fn discover(&self) -> Result<Vec<String>, FatalError> {
        let names = residuum_config::discover_agents(&self.services.root)?;
        for name in &names {
            self.adopt(name);
        }
        Ok(names)
    }

    pub fn adopt(&self, name: &str) {
        let dir = residuum_config::paths::agent_dir(&self.services.root, name);
        let mut slots = self.slots.write().unwrap_or_else(PoisonError::into_inner);
        if slots.contains_key(name) {
            return;
        }
        let slot = Arc::new(AgentSlot {
            name: name.to_string(),
            dir,
            op_lock: tokio::sync::Mutex::new(()),
            removed: AtomicBool::new(false),
            state: Mutex::new(SlotState {
                state: AgentState::Stopped,
                last_error: None,
                running: None,
                generation: 0,
                published_meta: None,
                meta_unreadable: false,
            }),
            activity: ActivityTracker::new(
                name,
                self.events.clone(),
                Arc::clone(&self.agent_changes),
            ),
        });
        slot.reload_meta();
        slots.insert(name.to_string(), slot);
    }

    pub fn forget(&self, name: &str) {
        self.slots
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(name);
        self.release_teams_port(name);
    }

    pub fn begin_shutdown(&self) {
        self.stopping.store(true, Ordering::SeqCst);
    }

    fn ensure_not_stopping(&self) -> Result<(), LifecycleError> {
        if self.stopping.load(Ordering::SeqCst) {
            return Err(LifecycleError::ShuttingDown(SHUTTING_DOWN.to_string()));
        }
        Ok(())
    }

    pub async fn start_autostart(&self) {
        let starts = self
            .slots()
            .into_iter()
            .filter(|slot| slot.reload_meta().0.autostart)
            .map(|slot| async move {
                let _op = slot.op_lock.lock().await;
                self.start_locked(&slot).await.ok();
            });
        futures_util::future::join_all(starts).await;
    }

    pub async fn stop_all(&self) {
        let stops = self.slots().into_iter().map(|slot| async move {
            slot.activity.take_restart_pending();
            let _op = slot.op_lock.lock().await;
            self.stop_locked(&slot).await.ok();
        });
        futures_util::future::join_all(stops).await;
    }

    pub fn hub_config_changed(&self, hub_cfg: HubConfig) {
        *self.hub_cfg.write().unwrap_or_else(PoisonError::into_inner) = hub_cfg;
        self.signal_running_agents(ReloadSignal::Hub, "the hub config change");
    }

    pub fn team_files_changed(&self) {
        self.signal_running_agents(ReloadSignal::Workspace, "the team file change");
    }

    fn signal_running_agents(&self, signal: ReloadSignal, about: &str) {
        for slot in self.slots() {
            if let Some(running) = &slot.lock().running
                && running.control.reload_tx.send(signal).is_err()
            {
                tracing::warn!(agent = %slot.name, "couldn't tell a running agent about {about}: its reload channel is closed");
            }
        }
    }

    #[must_use]
    pub fn workspace_watch_health(
        &self,
        name: &str,
    ) -> Option<watch::Receiver<residuum_workspace::watch::WatchHealth>> {
        let slot = self.slot(name).ok()?;
        let state = slot.lock();
        state
            .running
            .as_ref()
            .map(|running| running.control.workspace_watch_health.clone())
    }

    #[must_use]
    pub fn connected_clients(&self, name: &str) -> Option<usize> {
        self.slot(name)
            .ok()
            .map(|slot| slot.activity.connected_clients())
    }

    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn reloads_finished(&self, name: &str) -> Option<u64> {
        let slot = self.slot(name).ok()?;
        let state = slot.lock();
        state
            .running
            .as_ref()
            .map(|running| *running.control.reload_done.borrow())
    }

    #[must_use]
    pub fn active_subagents(&self) -> Vec<residuum_tracing_service::Subagent> {
        let registries: Vec<_> = self
            .slots()
            .iter()
            .filter_map(|slot| {
                slot.lock()
                    .running
                    .as_ref()
                    .map(|running| Arc::clone(&running.control.session_registry))
            })
            .collect();
        registries
            .iter()
            .flat_map(|registry| registry.subagent_snapshot())
            .collect()
    }

    fn summary_of(&self, slot: &AgentSlot) -> AgentSummary {
        let (state, last_error) = {
            let guard = slot.lock();
            (guard.state, guard.last_error.clone())
        };
        let meta = slot.current_meta();
        AgentSummary {
            name: slot.name.clone(),
            display_name: meta.display_name,
            state,
            last_error,
            autostart: meta.autostart,
            role: role_line(&self.team_paths(), &slot.name),
            a2a_visibility: meta.a2a_visibility,
            teams_configured: meta.teams_configured,
        }
    }

    fn publish_state(&self, slot: &AgentSlot) {
        self.publish(HubEvent::AgentState {
            agent: self.summary_of(slot),
        });
    }

    fn set_state(&self, slot: &AgentSlot, state: AgentState, last_error: Option<AgentLastError>) {
        {
            let mut guard = slot.lock();
            guard.state = state;
            guard.last_error = last_error;
            if state != AgentState::Running {
                guard.running = None;
            }
        }
        if matches!(state, AgentState::Stopped | AgentState::Failed) {
            self.release_teams_port(&slot.name);
        }
        slot.reload_meta();
        self.publish_state(slot);
    }

    fn refresh_meta(&self, slot: &AgentSlot) {
        let (_meta, changed) = slot.reload_meta();
        if changed {
            self.publish_state(slot);
        }
    }

    async fn start_locked(&self, slot: &Arc<AgentSlot>) -> Result<(), LifecycleError> {
        slot.ensure_present()?;
        self.ensure_not_stopping()?;
        if matches!(
            slot.lock().state,
            AgentState::Running | AgentState::Starting
        ) {
            return Ok(());
        }
        self.set_state(slot, AgentState::Starting, None);
        let mut reset_on_drop = StartingResetGuard {
            host: self,
            slot,
            armed: true,
        };

        let hub_cfg = self.hub_config();
        let preview = agent_span(&slot.name)
            .in_scope(|| Config::load_agent_at(&slot.dir, &hub_cfg))
            .ok();
        let teams_port = preview
            .as_ref()
            .and_then(|cfg| cfg.teams.as_ref())
            .map(|teams| teams.port);
        if let Some(port) = teams_port
            && let Err(other) = self.reserve_teams_port(&slot.name, port)
        {
            let message = format!(
                "{} can't start: its Teams adapter uses port {port}, which the agent '{other}' is already using. Give one of them a different Teams port, then start it again.",
                slot.name
            );
            let reason = format!("Teams port {port} is already used by the agent '{other}'");
            let failure = self
                .record_failure(
                    slot,
                    AgentErrorKind::PortConflict,
                    message,
                    &reason,
                    preview.as_ref(),
                )
                .await;
            reset_on_drop.armed = false;
            return Err(failure);
        }

        let inputs = AgentStartInputs {
            name: slot.name.clone(),
            agent_dir: slot.dir.clone(),
            services: self.services.clone(),
            hub_cfg,
            activity: Arc::clone(&slot.activity),
        };
        let launcher = Arc::clone(&self.launcher);
        let started = residuum_core::util::spawn_in_span(
            async move { launcher.start(inputs).await }.instrument(agent_span(&slot.name)),
        )
        .await;
        match started {
            Ok(Ok(started)) => {
                let watcher = self.attach_watcher(slot, &started.control).await;
                self.attach(slot, started, watcher);
                reset_on_drop.armed = false;
                Ok(())
            }
            Ok(Err(err)) => {
                let message = format!(
                    "{} couldn't start: {err}. Fix its settings or model configuration, then start it again.",
                    slot.name
                );
                let kind = match &err {
                    FatalError::Config(_) => AgentErrorKind::Config,
                    FatalError::Workspace(_) | FatalError::Gateway(_) | FatalError::Other(_) => {
                        AgentErrorKind::Other
                    }
                };
                let failure = self
                    .record_failure(slot, kind, message, &err.to_string(), preview.as_ref())
                    .await;
                reset_on_drop.armed = false;
                Err(failure)
            }
            Err(join_err) => {
                let detail = describe_join_error(join_err);
                let message = format!(
                    "{} hit an internal error while starting. Try starting it again; if it keeps happening, send a bug report.",
                    slot.name
                );
                let failure = self
                    .record_failure(
                        slot,
                        AgentErrorKind::Crash,
                        message,
                        &detail,
                        preview.as_ref(),
                    )
                    .await;
                reset_on_drop.armed = false;
                Err(failure)
            }
        }
    }

    fn reserve_teams_port(&self, agent: &str, port: u16) -> Result<(), String> {
        let mut ports = self
            .teams_ports
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(holder) = ports.get(&port)
            && holder != agent
        {
            return Err(holder.clone());
        }
        ports.retain(|_, holder| holder != agent);
        ports.insert(port, agent.to_string());
        self.publish_teams_ports(&ports);
        Ok(())
    }

    fn release_teams_port(&self, agent: &str) {
        let mut ports = self
            .teams_ports
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        ports.retain(|_, holder| holder != agent);
        self.publish_teams_ports(&ports);
    }

    pub fn subscribe_teams_ports(&self) -> watch::Receiver<BTreeMap<String, u16>> {
        self.teams_ports_tx.subscribe()
    }

    fn publish_teams_ports(&self, ports: &BTreeMap<u16, String>) {
        let by_agent = ports
            .iter()
            .map(|(port, agent)| (agent.clone(), *port))
            .collect();
        self.teams_ports_tx.send_if_modified(|current| {
            if *current == by_agent {
                false
            } else {
                *current = by_agent;
                true
            }
        });
    }

    pub fn refresh_teams_port(&self, slot: &AgentSlot) {
        let hub_cfg = self.hub_config();
        let cfg = match agent_span(&slot.name)
            .in_scope(|| Config::load_agent_at(&slot.dir, &hub_cfg))
        {
            Ok(cfg) => cfg,
            Err(e) => {
                tracing::warn!(agent = %slot.name, error = %e, "couldn't read the reloaded config to update the agent's Teams port");
                return;
            }
        };
        match cfg.teams.as_ref().map(|teams| teams.port) {
            Some(port) => {
                if let Err(other) = self.reserve_teams_port(&slot.name, port) {
                    tracing::warn!(agent = %slot.name, port, holder = %other, "the agent's Teams port is held by another agent");
                }
            }
            None => self.release_teams_port(&slot.name),
        }
    }

    async fn record_failure(
        &self,
        slot: &Arc<AgentSlot>,
        kind: AgentErrorKind,
        message: String,
        reason: &str,
        cfg: Option<&Config>,
    ) -> LifecycleError {
        tracing::error!(agent = %slot.name, error = %reason, "agent failed");
        self.services
            .tracing_service
            .on_error(
                &format!("agent failed: {reason}"),
                residuum_tracing_service::client_context::gather_for_agent(
                    self.services.control.build,
                    &slot.name,
                    cfg,
                ),
            )
            .await;
        self.set_state(
            slot,
            AgentState::Failed,
            Some(AgentLastError {
                message: message.clone(),
                kind,
                reason: reason.to_string(),
                at: Utc::now(),
            }),
        );
        LifecycleError::Failed(message)
    }

    async fn attach_watcher(
        &self,
        slot: &AgentSlot,
        control: &AgentControl,
    ) -> Option<AgentWatcher> {
        match AgentWatcher::attach(&slot.name, control, Arc::clone(&self.agent_changes)).await {
            Ok(watcher) => Some(watcher),
            Err(e) => {
                tracing::error!(agent = %slot.name, error = %e, "couldn't start watching the agent; the hub won't hear about its sessions, inbox or schedule until it restarts");
                None
            }
        }
    }

    fn attach(&self, slot: &Arc<AgentSlot>, started: StartedAgent, watcher: Option<AgentWatcher>) {
        let StartedAgent {
            control,
            event_loop,
        } = started;
        let join =
            residuum_core::util::spawn_in_span(event_loop.instrument(agent_span(&slot.name)));
        let abort = join.abort_handle();
        let (done_tx, done_rx) = watch::channel(false);
        let stop_requested = Arc::new(AtomicBool::new(false));
        let forced = Arc::new(AtomicBool::new(false));
        let generation = {
            let mut guard = slot.lock();
            guard.generation += 1;
            guard.running = Some(RunningAgent {
                control: control.clone(),
                stop_requested: Arc::clone(&stop_requested),
                forced: Arc::clone(&forced),
                done: done_rx,
                abort,
            });
            guard.generation
        };
        self.set_state(slot, AgentState::Running, None);
        self.agent_changes.publish(&AgentChange {
            agent: slot.name.clone(),
            kind: AgentChangeKind::Resync,
        });

        if let Some(host) = self.me.upgrade() {
            let supervised = Supervised {
                slot: Arc::clone(slot),
                generation,
                stop_requested,
                forced,
                done_tx,
                cleanup: control.cleanup.clone(),
                watcher,
            };
            residuum_core::util::spawn_in_span(
                {
                    let host = Arc::clone(&host);
                    async move { host.supervise(supervised, join).await }
                }
                .instrument(agent_span(&slot.name)),
            );
            let slot = Arc::clone(slot);
            let mut reload_done = control.reload_done;
            residuum_core::util::spawn_in_span(async move {
                while reload_done.changed().await.is_ok() {
                    host.refresh_meta(&slot);
                    host.refresh_teams_port(&slot);
                    host.spawn_team_embedding_refresh();
                }
            });
            self.spawn_team_embedding_refresh();
        }
    }

    pub(crate) async fn note_team_embedding(&self, source: Option<EmbeddingSource>) {
        *self.team_embedding.lock().await = source;
    }

    fn spawn_team_embedding_refresh(&self) {
        if let Some(host) = self.me.upgrade() {
            residuum_core::util::spawn_in_span(async move { host.refresh_team_embedding().await });
        }
    }

    pub async fn refresh_team_embedding(&self) {
        let mut current = self.team_embedding.lock().await;
        let hub_cfg = self.hub_config();
        let mut wanted = None;
        let mut unreadable_before_choice = false;
        for slot in self.slots() {
            match agent_span(&slot.name).in_scope(|| Config::load_agent_at(&slot.dir, &hub_cfg)) {
                Ok(cfg) => {
                    wanted = EmbeddingSource::from_config(&slot.name, &cfg);
                    if wanted.is_some() {
                        break;
                    }
                }
                Err(e) => {
                    tracing::debug!(agent = %slot.name, error = %e, "an agent's config doesn't load while choosing the team wiki's embedding model");
                    unreadable_before_choice = true;
                }
            }
        }
        if unreadable_before_choice {
            return;
        }
        let unchanged = match (&wanted, &*current) {
            (Some(new), Some(old)) => new.same_as(old),
            (None, None) => true,
            _ => false,
        };
        if unchanged {
            return;
        }
        let provider = match &wanted {
            Some(source) => match source.build() {
                Ok(provider) => Some(provider),
                Err(reason) => {
                    tracing::warn!(agent = %source.agent(), error = %reason, "the team wiki's embedding provider is unavailable; wiki search is text only");
                    self.notice(
                        NoticeLevel::Warn,
                        format!(
                            "Team wiki search is text-only: the embedding model {} configures can't be used ({reason}).",
                            source.agent()
                        ),
                        Some(source.agent().to_string()),
                    );
                    None
                }
            },
            None => None,
        };
        let expects_vectors = provider.is_some();
        let hybrid = self.services.team_wiki.set_embedding(provider).await;
        if expects_vectors && !hybrid {
            self.notice(
                NoticeLevel::Warn,
                "Team wiki search is text-only: its embedding store couldn't be opened. Check the log for the reason.".to_string(),
                None,
            );
        }
        tracing::info!(
            agent = wanted.as_ref().map_or("none", EmbeddingSource::agent),
            hybrid,
            "team wiki embedding model changed"
        );
        *current = wanted;
    }

    async fn supervise(&self, mut run: Supervised, join: tokio::task::JoinHandle<AgentExit>) {
        let exit = join.await;
        let requested = run.stop_requested.load(Ordering::SeqCst);
        let failure: Option<(String, String)> = match exit {
            Ok(AgentExit::Stopped) => None,
            Ok(AgentExit::BusClosed) => Some((
                format!(
                    "{} stopped unexpectedly. Restart it from Home; if it keeps happening, send a bug report.",
                    run.slot.name
                ),
                "the event loop ended because its message channel closed".to_string(),
            )),
            Err(ref join_err) if requested && join_err.is_cancelled() => None,
            Err(join_err) => Some((
                format!(
                    "{} crashed from an internal error. Restart it from Home; if it keeps happening, send a bug report.",
                    run.slot.name
                ),
                describe_join_error(join_err),
            )),
        };
        if failure.is_some() || run.forced.load(Ordering::SeqCst) {
            run.cleanup.run().await;
        }
        if let Some(watcher) = run.watcher.take() {
            watcher.stop().await;
        }
        run.slot.activity.run_ended();
        self.services
            .sibling_fanout
            .unregister(&run.slot.name)
            .await;
        self.services.team_router.unregister(&run.slot.name);

        let current = run.slot.lock().generation == run.generation;
        if current {
            match failure {
                None => self.set_state(&run.slot, AgentState::Stopped, None),
                Some((message, reason)) => {
                    let cfg = Config::load_agent_at(&run.slot.dir, &self.hub_config()).ok();
                    self.record_failure(
                        &run.slot,
                        AgentErrorKind::Crash,
                        message,
                        &reason,
                        cfg.as_ref(),
                    )
                    .await;
                }
            }
        }
        run.done_tx.send(true).ok();
    }

    async fn stop_locked(&self, slot: &Arc<AgentSlot>) -> Result<(), LifecycleError> {
        slot.ensure_present()?;
        let Some((stop_tx, stop_requested, forced, mut done, abort)) =
            slot.lock().running.as_ref().map(|running| {
                (
                    running.control.stop_tx.clone(),
                    Arc::clone(&running.stop_requested),
                    Arc::clone(&running.forced),
                    running.done.clone(),
                    running.abort.clone(),
                )
            })
        else {
            slot.activity.take_restart_pending();
            return Ok(());
        };
        stop_requested.store(true, Ordering::SeqCst);
        self.services.team_router.unregister(&slot.name);
        self.publish(HubEvent::AgentStopping {
            name: slot.name.clone(),
        });
        stop_tx.send(()).await.ok();
        slot.activity.take_restart_pending();
        if tokio::time::timeout(STOP_TIMEOUT, done.wait_for(|finished| *finished))
            .await
            .is_ok()
        {
            return Ok(());
        }
        tracing::error!(agent = %slot.name, timeout_secs = STOP_TIMEOUT.as_secs(), "agent didn't stop in time, aborting its task");
        forced.store(true, Ordering::SeqCst);
        abort.abort();
        if tokio::time::timeout(ABORT_SETTLE, done.wait_for(|finished| *finished))
            .await
            .is_err()
        {
            tracing::error!(agent = %slot.name, "agent task still not finished after being aborted");
            self.set_state(slot, AgentState::Stopped, None);
        }
        Ok(())
    }

    async fn run_deferred_restart(self: Arc<Self>, slot: Arc<AgentSlot>, generation: u64) {
        let timeout = self.graceful_restart_timeout();
        let timed_out =
            tokio::time::timeout(timeout, slot.activity.wait_until_turn_ends_or_cancelled())
                .await
                .is_err();
        if timed_out {
            tracing::warn!(
                agent = %slot.name,
                timeout_secs = timeout.as_secs(),
                "graceful restart timed out waiting for the active turn to finish; restarting immediately"
            );
            self.notice(
                NoticeLevel::Warn,
                format!(
                    "{}'s turn was still running after {}s, so its pending restart ran immediately.",
                    slot.current_meta().display_name,
                    timeout.as_secs()
                ),
                Some(slot.name.clone()),
            );
        }
        let _op = slot.op_lock.lock().await;
        if self.ensure_not_stopping().is_err() || slot.is_removed() {
            slot.activity.take_restart_pending();
            return;
        }
        let still_current = {
            let guard = slot.lock();
            guard.state == AgentState::Running && guard.generation == generation
        };
        if !still_current || !slot.activity.snapshot().restart_pending {
            slot.activity.take_restart_pending();
            return;
        }
        tracing::info!(
            agent = %slot.name,
            timed_out,
            "executing deferred restart after turn ended"
        );
        if let Err(e) = self.stop_locked(&slot).await {
            tracing::error!(agent = %slot.name, error = %e, "deferred restart failed while stopping agent");
            return;
        }
        let started = self.start_locked(&slot).await;
        self.note_start_outcome(&slot, &started);
    }

    async fn create_agent(
        &self,
        request: CreateAgentRequest,
        by: Actor,
    ) -> Result<AgentSummary, LifecycleError> {
        self.ensure_not_stopping()?;
        let display_name = residuum_config::canonicalize_display_name(&request.name)
            .map_err(LifecycleError::InvalidName)?;
        let from_slug = match &request.models_from {
            Some(from) => Some(self.slot(from)?.name.clone()),
            None => None,
        };
        let providers_toml = match (from_slug, &request.providers_toml) {
            (Some(from), _) => super::provision::copy_providers_from(&self.services.root, &from)?,
            (None, Some(raw)) => raw.clone(),
            (None, None) => {
                return Err(LifecycleError::InvalidRequest(
                    "a new agent needs model settings: name an existing agent in models_from, or include providers_toml".to_string(),
                ));
            }
        };
        let slug = {
            let _creating = self.creation_lock.lock().await;
            let slug = self.choose_slug(&display_name).await?;
            self.ensure_name_free(&slug)?;
            let spec = super::provision::AgentSpec {
                name: slug.clone(),
                display_name: display_name.clone(),
                providers_toml,
                a2a_visibility: request.a2a_visibility.unwrap_or(A2aVisibility::Private),
                description: request.description.clone(),
            };
            super::provision::provision_agent(
                &self.services.root,
                &self.team_paths(),
                &self.services.team,
                &team_writer(&by),
                &spec,
            )
            .await?;
            super::deleted::clear_deletion(&self.checkpoints_dir(), &slug).await;
            self.adopt(&slug);
            slug
        };

        let slot = self.slot(&slug)?;
        let started = {
            let _op = slot.op_lock.lock().await;
            self.start_locked(&slot).await
        };
        self.note_start_outcome(&slot, &started);
        if started.is_ok() {
            self.hand_over_role(
                &slot,
                &by,
                request.description.as_deref(),
                request.creator_hop,
            )
            .await;
        }
        let summary = self.summary_of(&slot);
        self.publish(HubEvent::AgentCreated {
            agent: summary.clone(),
            by,
        });
        Ok(summary)
    }

    pub fn note_start_outcome(&self, slot: &AgentSlot, started: &Result<(), LifecycleError>) {
        if let Err(reason) = started {
            tracing::info!(
                agent = %slot.name,
                state = %self.summary_of(slot).state,
                reason = %reason,
                "agent was not started"
            );
        }
    }

    async fn hand_over_role(
        &self,
        slot: &AgentSlot,
        by: &Actor,
        description: Option<&str>,
        hop_count: u32,
    ) {
        let Some(description) = description.map(str::trim).filter(|text| !text.is_empty()) else {
            return;
        };
        let control = slot
            .lock()
            .running
            .as_ref()
            .map(|running| running.control.clone());
        let Some(control) = control else {
            return;
        };
        let message = super::provision::first_message(description);
        if let Err(reason) = control
            .main_delivery
            .deliver_to_main(by, message, self.hub_config().timezone, hop_count)
            .await
        {
            tracing::warn!(agent = %slot.name, error = %reason, "couldn't deliver the new agent's role description");
            self.notice(
                NoticeLevel::Warn,
                format!(
                    "{} was created, but its role description couldn't be delivered ({reason}). Send it your description of its role yourself.",
                    slot.name
                ),
                Some(slot.name.clone()),
            );
        }
    }

    async fn delete_agent(&self, name: &str, by: Actor) -> Result<DeleteOutcome, LifecycleError> {
        let slot = self.slot(name)?;
        let slug = slot.name.clone();
        let display_name = self.summary_of(&slot).display_name;
        slot.activity.take_restart_pending();
        let checkpoint_id = {
            let _op = slot.op_lock.lock().await;
            slot.activity.take_restart_pending();
            self.stop_locked(&slot).await?;
            let mut record = super::deleted::DeletionRecord {
                deleted_at: Utc::now(),
                checkpoint_id: None,
                role_page: tokio::fs::read_to_string(self.team_paths().agent_role_page(&slug))
                    .await
                    .ok(),
                display_name: Some(display_name),
            };
            let engine = self.checkpoint_engine(&slot).map_err(|e| {
                tracing::error!(agent = %slug, error = %e, "couldn't open the agent's checkpoint repositories to delete it");
                LifecycleError::Failed(format!(
                    "couldn't open {slug}'s checkpoint history, so it was not deleted: {e}"
                ))
            })?;
            let id = super::provision::deprovision_agent(
                &self.services.root,
                &self.team_paths(),
                &self.services.team,
                &team_writer(&by),
                &slug,
                &engine,
            )
            .await?;
            record.checkpoint_id.clone_from(&id);
            super::deleted::record_deletion(&self.checkpoints_dir(), &slug, &record).await;
            slot.removed.store(true, Ordering::SeqCst);
            self.forget(&slug);
            id
        };
        self.spawn_team_embedding_refresh();
        self.publish(HubEvent::AgentDeleted {
            name: slug.clone(),
            by,
        });
        Ok(DeleteOutcome {
            deleted: true,
            checkpoint_id,
        })
    }

    async fn choose_slug(&self, display: &str) -> Result<String, LifecycleError> {
        let key = residuum_config::display_name_key(display);
        for slot in self.slots() {
            let meta = slot.current_meta();
            if residuum_config::display_name_key(&meta.display_name) == key {
                return Err(LifecycleError::AlreadyExists(meta.display_name));
            }
        }
        let checkpoints = self.checkpoints_dir();
        let history = residuum_checkpoints::agents_with_history(&checkpoints)
            .map_err(|e| history_unreadable(&e))?;
        let mut taken = history.clone();
        for slot in self.slots() {
            taken.push(slot.name.clone());
        }
        for slug in &history {
            if self
                .slots
                .read()
                .unwrap_or_else(PoisonError::into_inner)
                .contains_key(slug)
            {
                continue;
            }
            let shown = super::deleted::read_deletion(&checkpoints, slug)
                .await
                .and_then(|record| record.display_name)
                .unwrap_or_else(|| slug.clone());
            if residuum_config::display_name_key(&shown) == key {
                return Ok(slug.clone());
            }
        }
        residuum_config::allocate_slug(&residuum_config::slug_base(display), |candidate| {
            taken.iter().any(|slug| slug == candidate)
        })
        .map_err(LifecycleError::Failed)
    }

    async fn resolve_deleted_name(&self, requested: &str) -> Result<String, LifecycleError> {
        let checkpoints = self.checkpoints_dir();
        let history = residuum_checkpoints::agents_with_history(&checkpoints)
            .map_err(|e| history_unreadable(&e))?;
        if residuum_config::validate_agent_name(requested).is_ok()
            && history.iter().any(|slug| slug == requested)
        {
            return Ok(requested.to_string());
        }
        let canonical = residuum_config::canonicalize_display_name(requested)
            .map_err(LifecycleError::InvalidName)?;
        let key = residuum_config::display_name_key(&canonical);
        for slug in &history {
            let shown = super::deleted::read_deletion(&checkpoints, slug)
                .await
                .and_then(|record| record.display_name)
                .unwrap_or_else(|| slug.clone());
            if residuum_config::display_name_key(&shown) == key {
                return Ok(slug.clone());
            }
        }
        Err(LifecycleError::NoDeletedAgent(requested.to_string()))
    }

    fn ensure_name_free(&self, name: &str) -> Result<(), LifecycleError> {
        let held = self
            .slots
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .contains_key(name);
        if held {
            return Err(LifecycleError::AlreadyExists(name.to_string()));
        }
        Ok(())
    }

    async fn restore_agent(
        &self,
        request: RestoreAgentRequest,
        by: Actor,
    ) -> Result<AgentSummary, LifecycleError> {
        self.ensure_not_stopping()?;
        if let Ok(slot) = self.slot(&request.name) {
            return Err(LifecycleError::AlreadyExists(
                slot.current_meta().display_name.clone(),
            ));
        }
        let name = self.resolve_deleted_name(&request.name).await?;
        {
            let _creating = self.creation_lock.lock().await;
            self.ensure_name_free(&name)?;
            let dir = residuum_config::paths::agent_dir(&self.services.root, &name);
            if tokio::fs::try_exists(dir.join("config").join("config.toml"))
                .await
                .unwrap_or(false)
            {
                return Err(LifecycleError::AlreadyExists(name));
            }
            if !residuum_checkpoints::agents_with_history(&self.checkpoints_dir())
                .map_err(|e| history_unreadable(&e))?
                .contains(&name)
            {
                return Err(LifecycleError::NoDeletedAgent(name));
            }
            let engine = self.checkpoint_engine_for(&name, &dir).map_err(|e| {
                tracing::error!(agent = %name, error = %e, "couldn't open the deleted agent's checkpoint repositories to restore it");
                LifecycleError::Failed(format!(
                    "couldn't open {name}'s checkpoint history, so it was not restored: {e}"
                ))
            })?;
            let record = super::deleted::read_deletion(&self.checkpoints_dir(), &name).await;
            let checkpoint_id = match request.checkpoint_id {
                Some(id) => {
                    engine
                        .show_checkpoint(residuum_checkpoints::RepoKind::Workspace, id.clone())
                        .await
                        .map_err(|e| {
                            LifecycleError::InvalidRequest(format!(
                                "{name} has no checkpoint '{id}' to restore from ({e})"
                            ))
                        })?;
                    id
                }
                None => {
                    pre_delete_checkpoint(&engine, &name, record.as_ref())
                        .await?
                        .ok_or_else(|| LifecycleError::NoDeletedAgent(name.clone()))?
                        .id
                }
            };
            super::provision::restore_agent(
                &self.services.root,
                &self.team_paths(),
                &self.services.team,
                &team_writer(&by),
                &name,
                &super::provision::RestoreSource {
                    checkpoints: &engine,
                    workspace_checkpoint: &checkpoint_id,
                    role_page: record.as_ref().and_then(|r| r.role_page.as_deref()),
                },
            )
            .await?;
            super::deleted::clear_deletion(&self.checkpoints_dir(), &name).await;
            self.adopt(&name);
        }

        let slot = self.slot(&name)?;
        if self.summary_of(&slot).autostart {
            let _op = slot.op_lock.lock().await;
            let started = self.start_locked(&slot).await;
            self.note_start_outcome(&slot, &started);
        }
        self.spawn_team_embedding_refresh();
        let summary = self.summary_of(&slot);
        self.publish(HubEvent::AgentRestored {
            agent: summary.clone(),
            by,
        });
        Ok(summary)
    }

    async fn deleted_agents(&self) -> Result<Vec<DeletedAgent>, LifecycleError> {
        let checkpoints_dir = self.checkpoints_dir();
        let names = residuum_checkpoints::agents_with_history(&checkpoints_dir)
            .map_err(|e| history_unreadable(&e))?;
        let mut deleted = Vec::new();
        for name in names {
            if residuum_config::validate_agent_name(&name).is_err() {
                continue;
            }
            let dir = residuum_config::paths::agent_dir(&self.services.root, &name);
            let exists = tokio::fs::try_exists(dir.join("config").join("config.toml"))
                .await
                .unwrap_or(true);
            let held = self
                .slots
                .read()
                .unwrap_or_else(PoisonError::into_inner)
                .contains_key(&name);
            if exists || held {
                continue;
            }
            let engine = match self.checkpoint_engine_for(&name, &dir) {
                Ok(engine) => engine,
                Err(e) => {
                    tracing::warn!(agent = %name, error = %e, "couldn't open a deleted agent's checkpoint repositories; leaving it out of the deleted list");
                    continue;
                }
            };
            let record = super::deleted::read_deletion(&checkpoints_dir, &name).await;
            let latest = match pre_delete_checkpoint(&engine, &name, record.as_ref()).await {
                Ok(Some(latest)) => latest,
                Ok(None) => continue,
                Err(e) => {
                    tracing::warn!(agent = %name, error = %e, "couldn't read a deleted agent's checkpoints; leaving it out of the deleted list");
                    continue;
                }
            };
            let (deleted_at, display_name) = match record {
                Some(record) => (
                    record.deleted_at,
                    record.display_name.unwrap_or_else(|| name.clone()),
                ),
                None => (latest.timestamp, name.clone()),
            };
            deleted.push(DeletedAgent {
                name,
                display_name,
                deleted_at,
                checkpoint_id: latest.id,
            });
        }
        deleted.sort_by(|a, b| {
            b.deleted_at
                .cmp(&a.deleted_at)
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok(deleted)
    }

    async fn apply_patch(
        &self,
        slot: &Arc<AgentSlot>,
        patch: &AgentPatch,
    ) -> Result<(), LifecycleError> {
        slot.ensure_present()?;
        let config_path = slot.dir.join("config").join("config.toml");
        let existing = tokio::fs::read_to_string(&config_path).await.map_err(|e| {
            tracing::error!(agent = %slot.name, error = %e, path = %config_path.display(), "couldn't read the agent's config to patch it");
            LifecycleError::Failed(format!(
                "couldn't read {}'s settings file: {e}",
                slot.name
            ))
        })?;

        let mut diff = serde_json::Map::new();
        if let Some(autostart) = patch.autostart {
            diff.insert("autostart".to_string(), serde_json::Value::Bool(autostart));
        }
        if let Some(visibility) = patch.a2a_visibility {
            diff.insert(
                "a2a".to_string(),
                serde_json::json!({ "visibility": visibility_wire(visibility) }),
            );
        }
        let patched = residuum_config::patch::apply_patch(&existing, &diff, "config.toml")
            .map_err(LifecycleError::Failed)?;
        Config::validate_agent_toml(&patched, &slot.dir, &slot.name, &self.hub_config())
            .map_err(|e| {
                LifecycleError::Failed(format!(
                    "{}'s settings would not be valid with that change, so nothing was changed: {e}",
                    slot.name
                ))
            })?;

        self.checkpoint_config_before_write(slot).await;
        residuum_core::util::fs::atomic_write(&config_path, &patched)
            .await
            .map_err(|e| {
                tracing::error!(agent = %slot.name, error = %e, path = %config_path.display(), "failed to write the patched agent config");
                LifecycleError::Failed(format!(
                    "couldn't save {}'s settings: {e}",
                    slot.name
                ))
            })?;

        let waiting = slot.lock().running.as_ref().map(|running| {
            (
                running.control.reload_tx.clone(),
                running.control.reload_done.clone(),
            )
        });
        if let Some((reload_tx, mut reload_done)) = waiting {
            let before = *reload_done.borrow();
            if reload_tx.send(ReloadSignal::Agent).is_err() {
                tracing::warn!(agent = %slot.name, "the settings were saved, but the running agent's reload channel is closed");
            } else if tokio::time::timeout(
                PATCH_RELOAD_TIMEOUT,
                reload_done.wait_for(|finished| *finished > before),
            )
            .await
            .is_err()
            {
                tracing::warn!(agent = %slot.name, timeout_secs = PATCH_RELOAD_TIMEOUT.as_secs(), "the agent hasn't finished reloading its new settings yet");
            }
        }
        self.refresh_meta(slot);
        Ok(())
    }

    async fn checkpoint_config_before_write(&self, slot: &AgentSlot) {
        match self.checkpoint_engine(slot) {
            Ok(engine) => {
                let _checkpoint_id = engine
                    .checkpoint_config_kind_id_before_write(
                        residuum_checkpoints::RepoKind::AgentConfig,
                        residuum_checkpoints::CheckpointContext::system(
                            residuum_checkpoints::CheckpointTrigger::PreConfigWrite,
                            "patch config.toml",
                        ),
                    )
                    .await;
            }
            Err(e) => {
                tracing::warn!(agent = %slot.name, error = %e, "couldn't open the agent's checkpoint repositories; continuing without checkpointing this write");
            }
        }
    }

    fn checkpoint_engine(
        &self,
        slot: &AgentSlot,
    ) -> Result<Arc<residuum_checkpoints::CheckpointEngine>, residuum_checkpoints::CheckpointError>
    {
        self.checkpoint_engine_for(&slot.name, &slot.dir)
    }

    pub fn checkpoint_engine_for(
        &self,
        name: &str,
        dir: &Path,
    ) -> Result<Arc<residuum_checkpoints::CheckpointEngine>, residuum_checkpoints::CheckpointError>
    {
        self.checkpoint_opener(name, dir)()
    }

    fn checkpoint_opener(
        &self,
        name: &str,
        dir: &Path,
    ) -> impl Fn() -> Result<
        Arc<residuum_checkpoints::CheckpointEngine>,
        residuum_checkpoints::CheckpointError,
    > + Send
    + Sync
    + 'static {
        let shared = Arc::clone(&self.services.checkpoints);
        let team = self.services.team.clone();
        let checkpoints_dir = self.checkpoints_dir();
        let name = name.to_string();
        let dir = dir.to_path_buf();
        move || {
            residuum_checkpoints::CheckpointEngine::with_shared_repos(
                Arc::clone(&shared),
                &name,
                dir.clone(),
                dir.join("config"),
                &checkpoints_dir,
                None,
            )
            .map(|engine| Arc::new(engine.with_team_coordinator(team.clone())))
        }
    }

    pub fn checkpoints_dir(&self) -> PathBuf {
        residuum_config::HubPaths::new(&self.services.hub_dir).checkpoints_dir()
    }

    fn repair_router_for(&self, slot: &AgentSlot) -> Result<Router, LifecycleError> {
        let checkpoints = self.checkpoint_engine(slot).map_err(|e| {
            tracing::error!(agent = %slot.name, error = %e, "couldn't open the agent's checkpoint repositories for its repair routes");
            LifecycleError::Failed(format!(
                "couldn't open {}'s checkpoint history: {e}",
                slot.name
            ))
        })?;
        let layout = WorkspaceLayout::new(&slot.dir);
        let reload_tx = slot
            .lock()
            .running
            .as_ref()
            .map(|running| running.control.reload_tx.clone());
        let state = crate::web::ConfigApiState {
            hub_dir: self.services.hub_dir.clone(),
            config_dir: slot.dir.join("config"),
            agent_name: slot.name.clone(),
            workspace_dir: slot.dir.clone(),
            memory_dir: Some(layout.memory_dir()),
            reload_tx,
            checkpoints,
            team: Some(self.services.team.view_for_user(&slot.dir)),
            scope: crate::web::WorkspaceScope::Agent,
        };
        Ok(crate::web::agent_repair_api_router(state))
    }

    fn file_router_for(&self, slot: &AgentSlot) -> Router {
        let reload_tx = slot
            .lock()
            .running
            .as_ref()
            .map(|running| running.control.reload_tx.clone());
        let layout = WorkspaceLayout::new(&slot.dir);
        let files = crate::web::agent_files_api_router(crate::web::AgentFilesState {
            agent_name: slot.name.clone(),
            workspace_dir: slot.dir.clone(),
            memory_dir: Some(layout.memory_dir()),
            file_registry: crate::file_server::FileRegistry::for_agent(slot.name.clone(), &layout),
            reload_tx,
            checkpoints: crate::web::CheckpointAccess::lazy(
                self.checkpoint_opener(&slot.name, &slot.dir),
            ),
        });
        let usage = crate::web::UsageState {
            recorder: UsageRecorder::new(&layout),
            prices: Arc::new(PriceTable::new(&self.hub_config().pricing)),
        };
        files.merge(crate::web::agent_usage_api_router(usage))
    }

    async fn run_in_host_task<T, F, Fut>(&self, op: F) -> Result<T, LifecycleError>
    where
        T: Send + 'static,
        F: FnOnce(Arc<Self>) -> Fut,
        Fut: std::future::Future<Output = Result<T, LifecycleError>> + Send + 'static,
    {
        let host = self
            .me
            .upgrade()
            .ok_or_else(|| LifecycleError::ShuttingDown(SHUTTING_DOWN.to_string()))?;
        residuum_core::util::spawn_in_span(op(host))
            .await
            .map_err(|join_err| {
                let detail = describe_join_error(join_err);
                tracing::error!(error = %detail, "agent lifecycle task panicked");
                LifecycleError::Failed(format!(
                    "internal error during agent lifecycle operation: {detail}"
                ))
            })?
    }
}

struct Supervised {
    slot: Arc<AgentSlot>,
    generation: u64,
    stop_requested: Arc<AtomicBool>,
    forced: Arc<AtomicBool>,
    done_tx: watch::Sender<bool>,
    cleanup: AgentCleanup,
    watcher: Option<AgentWatcher>,
}

fn history_unreadable(error: &residuum_checkpoints::CheckpointError) -> LifecycleError {
    tracing::error!(error = %error, "couldn't read the deleted agents' checkpoint history");
    LifecycleError::Failed(format!(
        "couldn't read the checkpoint history of deleted agents: {error}"
    ))
}

const DELETE_CHECKPOINT_SEARCH_DEPTH: usize = 50;

async fn pre_delete_checkpoint(
    engine: &residuum_checkpoints::CheckpointEngine,
    name: &str,
    record: Option<&super::deleted::DeletionRecord>,
) -> Result<Option<residuum_checkpoints::CheckpointSummary>, LifecycleError> {
    use residuum_checkpoints::{CheckpointTrigger, RepoKind};
    if let Some(id) = record.and_then(|r| r.checkpoint_id.clone()) {
        match engine
            .show_checkpoint(RepoKind::Workspace, id.clone())
            .await
        {
            Ok(detail) => return Ok(Some(detail.summary)),
            Err(e) => {
                tracing::warn!(agent = %name, checkpoint = %id, error = %e, "the checkpoint recorded for the agent's deletion can't be read; using its newest checkpoints instead");
            }
        }
    }
    let page = engine
        .list_checkpoints(
            RepoKind::Workspace,
            None,
            None,
            None,
            Some(DELETE_CHECKPOINT_SEARCH_DEPTH),
        )
        .await
        .map_err(|e| {
            tracing::error!(agent = %name, error = %e, "couldn't list a deleted agent's checkpoints");
            LifecycleError::Failed(format!("couldn't read {name}'s checkpoint history: {e}"))
        })?;
    let deletion_summary = format!("delete agent {name}");
    let mut items = page.items.into_iter();
    let newest = items.next();
    let deletion = newest
        .iter()
        .chain(items.as_slice())
        .find(|c| c.trigger == CheckpointTrigger::PreAction && c.summary == deletion_summary)
        .cloned();
    Ok(deletion.or(newest))
}

fn describe_join_error(error: tokio::task::JoinError) -> String {
    if error.is_panic() {
        let payload = error.into_panic();
        format!(
            "panicked: {}",
            residuum_core::util::panic_message(&*payload)
        )
    } else {
        format!("task ended abnormally: {error}")
    }
}

struct StartingResetGuard<'a> {
    host: &'a AgentHost,
    slot: &'a Arc<AgentSlot>,
    armed: bool,
}

impl Drop for StartingResetGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.host.set_state(self.slot, AgentState::Stopped, None);
        }
    }
}

fn team_writer(by: &Actor) -> TeamWriter {
    match by {
        Actor::User => TeamWriter::User,
        Actor::Agent(name) => TeamWriter::Agent(name.clone()),
    }
}

fn visibility_wire(visibility: A2aVisibility) -> &'static str {
    match visibility {
        A2aVisibility::Public => "public",
        A2aVisibility::Private => "private",
    }
}

#[async_trait]
impl TeamDirectory for AgentHost {
    fn list(&self) -> Vec<AgentSummary> {
        self.slots()
            .iter()
            .map(|slot| self.summary_of(slot))
            .collect()
    }

    fn summary(&self, name: &str) -> Result<AgentSummary, LifecycleError> {
        Ok(self.summary_of(&*self.slot(name)?))
    }

    fn agent_a2a_router(&self, name: &str) -> Result<Router, LifecycleError> {
        let slot = self.slot(name)?;
        let guard = slot.lock();
        match &guard.running {
            Some(running) if guard.state == AgentState::Running => {
                running.control.a2a_router.borrow().clone().ok_or_else(|| {
                    LifecycleError::Failed(format!(
                        "{name}'s A2A interface isn't available; check its log"
                    ))
                })
            }
            _ => Err(LifecycleError::NotRunning {
                name: name.to_string(),
                state: guard.state,
            }),
        }
    }

    async fn create(
        &self,
        request: CreateAgentRequest,
        by: Actor,
    ) -> Result<AgentSummary, LifecycleError> {
        self.run_in_host_task(move |host| async move { host.create_agent(request, by).await })
            .await
    }

    async fn delete(&self, name: &str, by: Actor) -> Result<DeleteOutcome, LifecycleError> {
        let name = name.to_string();
        self.run_in_host_task(move |host| async move {
            let result = host.delete_agent(&name, by.clone()).await;
            if let (Err(e), Actor::Agent(actor)) = (&result, &by)
                && *actor == name
            {
                tracing::error!(agent = %name, error = %e, "an agent's request to delete itself failed");
                host.notice(
                    NoticeLevel::Warn,
                    format!("{name} asked to be deleted, but that failed: {e}. It has been stopped; check its state on Home."),
                    Some(name),
                );
            }
            result
        })
        .await
    }
}

impl residuum_workspace::inbox::mail::MailboxDirectory for AgentHost {
    fn mailboxes(&self) -> Vec<residuum_workspace::inbox::mail::Mailbox> {
        crate::mail::AgentMailboxes(self).mailboxes()
    }

    fn mailbox(&self, agent: &str) -> Option<residuum_workspace::inbox::mail::Mailbox> {
        crate::mail::AgentMailboxes(self).mailbox(agent)
    }
}

#[async_trait]
impl AgentDirectory for AgentHost {
    fn agent_router(&self, name: &str) -> Result<Router, LifecycleError> {
        let slot = self.slot(name)?;
        let guard = slot.lock();
        match &guard.running {
            Some(running) if guard.state == AgentState::Running => {
                Ok(running.control.router.clone())
            }
            _ => Err(LifecycleError::NotRunning {
                name: name.to_string(),
                state: guard.state,
            }),
        }
    }

    fn agent_repair_router(&self, name: &str) -> Result<Router, LifecycleError> {
        let slot = self.slot(name)?;
        self.repair_router_for(&slot)
    }

    fn agent_file_router(&self, name: &str) -> Result<Router, LifecycleError> {
        let slot = self.slot(name)?;
        Ok(self.file_router_for(&slot))
    }

    fn agent_files(&self, name: &str) -> Result<AgentFiles, LifecycleError> {
        let slot = self.slot(name)?;
        Ok(AgentFiles {
            dir: slot.dir.clone(),
            timezone: self.hub_config().timezone,
        })
    }

    fn activity(&self) -> Vec<(String, AgentActivity)> {
        self.slots()
            .iter()
            .map(|slot| (slot.name.clone(), slot.activity.snapshot()))
            .collect()
    }

    fn stopping(&self) -> Vec<String> {
        self.slots()
            .iter()
            .filter(|slot| {
                slot.lock()
                    .running
                    .as_ref()
                    .is_some_and(|running| running.stop_requested.load(Ordering::SeqCst))
            })
            .map(|slot| slot.name.clone())
            .collect()
    }

    fn live_sessions(&self, name: &str) -> Vec<SessionInfo> {
        let Ok(slot) = self.slot(name) else {
            return Vec::new();
        };
        let registry = slot
            .lock()
            .running
            .as_ref()
            .map(|running| Arc::clone(&running.control.session_registry));
        registry
            .map(|registry| registry.list_live())
            .unwrap_or_default()
    }

    async fn list_deleted(&self) -> Result<Vec<DeletedAgent>, LifecycleError> {
        self.deleted_agents().await
    }

    async fn restore(
        &self,
        request: RestoreAgentRequest,
        by: Actor,
    ) -> Result<AgentSummary, LifecycleError> {
        self.run_in_host_task(move |host| async move { host.restore_agent(request, by).await })
            .await
    }

    async fn start(&self, name: &str) -> Result<AgentSummary, LifecycleError> {
        let slot = self.slot(name)?;
        self.run_in_host_task(move |host| async move {
            let _op = slot.op_lock.lock().await;
            host.start_locked(&slot).await?;
            Ok(host.summary_of(&slot))
        })
        .await
    }

    async fn stop(&self, name: &str) -> Result<AgentSummary, LifecycleError> {
        let slot = self.slot(name)?;
        slot.activity.take_restart_pending();
        self.run_in_host_task(move |host| async move {
            let _op = slot.op_lock.lock().await;
            slot.activity.take_restart_pending();
            host.stop_locked(&slot).await?;
            Ok(host.summary_of(&slot))
        })
        .await
    }

    async fn restart(&self, name: &str) -> Result<AgentSummary, LifecycleError> {
        self.ensure_not_stopping()?;
        let slot = self.slot(name)?;
        self.run_in_host_task(move |host| async move {
            let _op = slot.op_lock.lock().await;
            slot.ensure_present()?;
            host.ensure_not_stopping()?;
            let running_gen = {
                let guard = slot.lock();
                (guard.state == AgentState::Running).then_some(guard.generation)
            };
            if let Some(generation) = running_gen {
                let (deferred, newly_scheduled) = slot.activity.defer_restart_if_busy();
                if deferred {
                    if newly_scheduled {
                        tracing::info!(
                            agent = %slot.name,
                            "agent is busy with a main turn; deferring restart until the turn ends"
                        );
                        let host_for_task = Arc::clone(&host);
                        let slot_for_task = Arc::clone(&slot);
                        residuum_core::util::spawn_in_span(
                            async move {
                                host_for_task
                                    .run_deferred_restart(slot_for_task, generation)
                                    .await;
                            }
                            .instrument(agent_span(&slot.name)),
                        );
                    } else {
                        tracing::debug!(
                            agent = %slot.name,
                            "coalesced duplicate restart request into pending turn-end restart"
                        );
                    }
                    return Ok(host.summary_of(&slot));
                }
            }
            host.stop_locked(&slot).await?;
            host.start_locked(&slot).await?;
            Ok(host.summary_of(&slot))
        })
        .await
    }

    async fn restart_now(&self, name: &str) -> Result<AgentSummary, LifecycleError> {
        self.ensure_not_stopping()?;
        let slot = self.slot(name)?;
        slot.activity.take_restart_pending();
        self.run_in_host_task(move |host| async move {
            let _op = slot.op_lock.lock().await;
            slot.activity.take_restart_pending();
            host.stop_locked(&slot).await?;
            host.start_locked(&slot).await?;
            Ok(host.summary_of(&slot))
        })
        .await
    }

    async fn patch(&self, name: &str, patch: AgentPatch) -> Result<AgentSummary, LifecycleError> {
        if patch.is_empty() {
            return Err(LifecycleError::InvalidRequest(
                "the change must set autostart, a2a_visibility, or both".to_string(),
            ));
        }
        let slot = self.slot(name)?;
        let _op = slot.op_lock.lock().await;
        self.apply_patch(&slot, &patch).await?;
        Ok(self.summary_of(&slot))
    }

    fn subscribe(&self) -> broadcast::Receiver<HubEvent> {
        self.events.subscribe()
    }
}
