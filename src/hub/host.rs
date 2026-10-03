//! The agent host: the hub's set of agents and their lifecycle.
//!
//! [`AgentHost`] implements [`AgentDirectory`]. It scans `~/.residuum/` for
//! agent directories, starts the `autostart` ones with the hub, and tracks
//! each agent's [`AgentState`] and last error. Each agent's event loop runs as
//! its own task, watched by a supervisor: a panic or fatal error moves that
//! agent alone to `failed` with a plain-language message, while the hub and
//! the other agents keep running.
//!
//! Every state, autostart, visibility, and activity change is published on
//! the hub bus, in order, for the hub WebSocket to forward. What happens
//! inside a running agent (its sessions, inbox, schedule and turns) reaches
//! the hub's [`AgentChangeFeed`] through the agent's watcher, which starts
//! and stops with the agent.

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

use crate::background::registry::SessionInfo;
use crate::config::paths::TeamPaths;
use crate::config::{Config, HubConfig};
use crate::gateway::ReloadSignal;
use crate::gateway::event_loop::{
    AgentCleanup, AgentControl, AgentExit, AgentStartInputs, agent_span, spawn_agent_loop,
    start_agent,
};
use crate::util::FatalError;
use crate::workspace::layout::WorkspaceLayout;

use super::activity::ActivityTracker;
use super::agent_watch::{AgentChange, AgentChangeFeed, AgentChangeKind, AgentWatcher};
use super::directory::{AgentDirectory, AgentFiles};
use super::services::HubServices;
use super::team_embedding::EmbeddingSource;
use super::types::{
    A2aVisibility, Actor, AgentActivity, AgentErrorKind, AgentLastError, AgentPatch, AgentState,
    AgentSummary, CreateAgentRequest, DeleteOutcome, DeletedAgent, HubEvent, LifecycleError,
    NoticeLevel, RestoreAgentRequest,
};
use crate::workspace::team_files::TeamWriter;

/// Capacity of the hub bus. A subscriber that falls this far behind loses the
/// oldest events and is told so; the hub WebSocket resends a snapshot then.
const HUB_EVENT_CAPACITY: usize = 256;

/// How long a stop waits for an agent to wind down (post-turn cycles, live
/// sessions, adapters, MCP servers) before its task is aborted.
const STOP_TIMEOUT: Duration = Duration::from_secs(150);

/// How long a stop waits after aborting the agent's task.
const ABORT_SETTLE: Duration = Duration::from_secs(10);

/// How long `patch` waits for the agent to finish reloading the config it
/// just wrote before answering anyway.
const PATCH_RELOAD_TIMEOUT: Duration = Duration::from_secs(30);

/// What start, restart, and create answer once the hub is shutting down.
const SHUTTING_DOWN: &str = "Residuum is shutting down";

/// Whether `doc`'s `[teams]` section has the three fields the listener requires.
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

/// Settings the summary reads from the agent's config file: whether it
/// starts with the hub, who can see it, and the name people use for it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AgentMeta {
    autostart: bool,
    a2a_visibility: A2aVisibility,
    display_name: String,
    /// A complete `[teams]` section: app id, tenant id, and client secret all set.
    teams_configured: bool,
}

impl AgentMeta {
    /// What a summary reports for an agent whose config has never loaded:
    /// the default autostart, private (so a card that can't be checked is
    /// never served without a key), and the folder name as the shown name.
    fn unverified(name: &str) -> Self {
        Self {
            autostart: true,
            a2a_visibility: A2aVisibility::Private,
            display_name: name.to_string(),
            teams_configured: false,
        }
    }

    /// Read `autostart`, `[a2a] visibility`, and `display_name` from the
    /// agent's `config/config.toml`, with the defaults an absent key resolves
    /// to. `had_meta` says whether a config has loaded before. `name` is the
    /// folder name, shown when `display_name` is absent or unusable.
    ///
    /// # Errors
    /// Returns why the file could not be used: it can't be read, isn't valid
    /// TOML, or is empty when a config has loaded before (as a half-written
    /// file is). The caller keeps the last meta that did load rather than
    /// guessing. An empty file for an agent that never loaded one reads as
    /// all defaults.
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
            Some(raw) => match crate::config::canonicalize_display_name(raw) {
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

/// The `description` frontmatter of a role page.
#[derive(Deserialize)]
struct RoleFrontmatter {
    description: Option<String>,
}

/// The one-line role from `team/wiki/agents/<name>.md`, if the page exists
/// and has a description.
fn role_line(team: &TeamPaths, name: &str) -> Option<String> {
    let page = team.agent_role_page(name);
    let text = std::fs::read_to_string(&page).ok()?;
    match crate::util::parse_frontmatter_md::<RoleFrontmatter>(&text, "agent role page") {
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

/// Everything the host keeps for an agent that is running.
struct RunningAgent {
    /// The routers, reload and stop channels, and cleanup the agent started
    /// with.
    control: AgentControl,
    /// Set before the stop signal is sent, so the supervisor can tell a
    /// requested stop from a failure.
    stop_requested: Arc<AtomicBool>,
    /// Set before the agent's task is aborted after a stop timed out.
    forced: Arc<AtomicBool>,
    /// Turns `true` once the supervisor has recorded the agent's exit.
    done: watch::Receiver<bool>,
    abort: AbortHandle,
}

struct SlotState {
    state: AgentState,
    last_error: Option<AgentLastError>,
    running: Option<RunningAgent>,
    generation: u64,
    /// Autostart and visibility from the last config that loaded, as last
    /// published. `None` until one has loaded. Summaries and the A2A
    /// listener's auth read this, so a config that is broken or half written
    /// never changes what they report.
    published_meta: Option<AgentMeta>,
    /// Whether the config file could not be used the last time it was read,
    /// so the problem is logged once per streak instead of on every request.
    meta_unreadable: bool,
}

/// One agent the host knows about.
struct AgentSlot {
    name: String,
    dir: PathBuf,
    /// Serializes start, stop, restart, and patch on this agent.
    op_lock: tokio::sync::Mutex<()>,
    /// Set under `op_lock` once the agent has been deleted. Anything that
    /// took this slot before the delete and gets `op_lock` afterwards sees it
    /// and stops, so a deleted agent's directory is never written again.
    removed: AtomicBool,
    state: Mutex<SlotState>,
    activity: Arc<ActivityTracker>,
}

impl AgentSlot {
    fn lock(&self) -> std::sync::MutexGuard<'_, SlotState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn is_removed(&self) -> bool {
        self.removed.load(Ordering::SeqCst)
    }

    /// The error for an operation that reached a deleted agent's slot.
    fn ensure_present(&self) -> Result<(), LifecycleError> {
        if self.is_removed() {
            return Err(LifecycleError::NotFound(self.name.clone()));
        }
        Ok(())
    }

    /// Re-read the config file. A file that can't be used leaves the last
    /// meta that loaded in place (private if none ever has). Returns the meta
    /// to report and whether it differs from the one published before.
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

    /// The settings to report: while the agent runs, those of the config it
    /// last loaded; otherwise the config file, falling back to the last that
    /// loaded.
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

/// The hub's agents: lookup, per-agent routing, and lifecycle.
pub struct AgentHost {
    me: Weak<Self>,
    services: HubServices,
    hub_cfg: RwLock<HubConfig>,
    slots: RwLock<BTreeMap<String, Arc<AgentSlot>>>,
    events: broadcast::Sender<HubEvent>,
    /// Changes inside running agents, published by each agent's watcher and
    /// by its activity tracker's turn hook.
    agent_changes: Arc<AgentChangeFeed>,
    /// Serializes creating agents, so two requests for one name can't both
    /// write its directory.
    creation_lock: tokio::sync::Mutex<()>,
    /// Set when the hub begins shutting down; from then on nothing starts.
    stopping: AtomicBool,
    /// Teams adapter ports held by agents that are starting or running,
    /// keyed by port, so two agents starting at once can't both take one.
    teams_ports: Mutex<BTreeMap<u16, String>>,
    /// The same reservations keyed by agent name, for the tunnel to dial the
    /// listener a Teams message names. Updated under the same changes as
    /// [`Self::teams_ports`].
    teams_ports_tx: watch::Sender<BTreeMap<String, u16>>,
    /// The embedding model the team wiki currently uses. Holding the lock
    /// covers choosing and swapping, so refreshes never interleave.
    team_embedding: tokio::sync::Mutex<Option<EmbeddingSource>>,
}

impl AgentHost {
    /// A host over `services`, knowing no agents yet; call
    /// [`Self::discover`] to scan for them. `hub_cfg` is the hub config
    /// every agent resolves its own config against.
    pub(crate) fn new(services: HubServices, hub_cfg: HubConfig) -> Arc<Self> {
        let (events, _first_subscriber) = broadcast::channel(HUB_EVENT_CAPACITY);
        let (teams_ports_tx, _) = watch::channel(BTreeMap::new());
        Arc::new_cyclic(|me| {
            // Agent tools and the team router reach this host through the
            // directory handle in the shared services.
            services
                .directory
                .bind(Weak::clone(me) as Weak<dyn AgentDirectory>);
            Self {
                me: Weak::clone(me),
                services,
                hub_cfg: RwLock::new(hub_cfg),
                slots: RwLock::new(BTreeMap::new()),
                events,
                agent_changes: AgentChangeFeed::new(),
                creation_lock: tokio::sync::Mutex::new(()),
                stopping: AtomicBool::new(false),
                teams_ports: Mutex::new(BTreeMap::new()),
                teams_ports_tx,
                team_embedding: tokio::sync::Mutex::new(None),
            }
        })
    }

    fn slots(&self) -> Vec<Arc<AgentSlot>> {
        self.slots
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .cloned()
            .collect()
    }

    fn slot(&self, name: &str) -> Result<Arc<AgentSlot>, LifecycleError> {
        let slots = self.slots.read().unwrap_or_else(PoisonError::into_inner);
        if let Some(slot) = slots.get(name) {
            return Ok(Arc::clone(slot));
        }
        // A typed name (`Research Desk`, or `atlas` for an agent shown as
        // `Atlas`) resolves to the folder it belongs to. The folder name
        // itself was returned above, so a folder is never hidden by another
        // agent's shown name.
        let Ok(canonical) = crate::config::canonicalize_display_name(name) else {
            return Err(LifecycleError::NotFound(name.to_string()));
        };
        let key = crate::config::display_name_key(&canonical);
        let mut found = None;
        for slot in slots.values() {
            if crate::config::display_name_key(&slot.current_meta().display_name) != key {
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

    fn team_paths(&self) -> TeamPaths {
        TeamPaths::new(crate::config::paths::team_dir(&self.services.root))
    }

    fn hub_config(&self) -> HubConfig {
        self.hub_cfg
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Publish an event on the hub bus.
    pub fn publish(&self, event: HubEvent) {
        // No subscribers is the normal state until a client opens the hub
        // WebSocket.
        self.events.send(event).ok();
    }

    /// The feed of changes inside running agents: sessions, outbound tasks,
    /// user inbox additions, watched files, turns that ended, and resyncs.
    /// Subscribe before agents start to hear about every one of them.
    #[must_use]
    pub fn agent_changes(&self) -> &Arc<AgentChangeFeed> {
        &self.agent_changes
    }

    /// Publish a hub notice for the user.
    pub fn notice(&self, level: NoticeLevel, message: String, agent: Option<String>) {
        self.publish(HubEvent::Notice {
            level,
            message,
            agent,
        });
    }

    /// Register the agents found under `~/.residuum/` as stopped agents,
    /// keeping the ones already known. Returns the names found, sorted.
    ///
    /// # Errors
    /// Returns `FatalError::Config` if the residuum root cannot be scanned.
    pub fn discover(&self) -> Result<Vec<String>, FatalError> {
        let names = crate::config::discover_agents(&self.services.root)?;
        for name in &names {
            self.adopt(name);
        }
        Ok(names)
    }

    /// Register the agent directory `~/.residuum/<name>` as a stopped agent,
    /// if it isn't already known. A newly created agent is adopted once its
    /// directory is written, then started.
    pub fn adopt(&self, name: &str) {
        let dir = crate::config::paths::agent_dir(&self.services.root, name);
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

    /// Forget the agent `name`, whose directory has been removed. The agent
    /// must already be stopped.
    pub fn forget(&self, name: &str) {
        self.slots
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(name);
        self.release_teams_port(name);
    }

    /// From now on nothing starts: `start`, `restart`, and `create` refuse
    /// with a plain-language error. The hub calls this before it stops the
    /// agents and the servers, so a request arriving in between can't start
    /// an agent that nothing would stop.
    pub fn begin_shutdown(&self) {
        self.stopping.store(true, Ordering::SeqCst);
    }

    fn ensure_not_stopping(&self) -> Result<(), LifecycleError> {
        if self.stopping.load(Ordering::SeqCst) {
            return Err(LifecycleError::ShuttingDown(SHUTTING_DOWN.to_string()));
        }
        Ok(())
    }

    /// Start every agent whose `autostart` is on, concurrently. A failure to
    /// start one is that agent's `failed` state, not an error here.
    pub async fn start_autostart(&self) {
        let starts = self
            .slots()
            .into_iter()
            .filter(|slot| slot.reload_meta().0.autostart)
            .map(|slot| async move {
                let _op = slot.op_lock.lock().await;
                // The failure is recorded on the agent and logged there.
                self.start_locked(&slot).await.ok();
            });
        futures_util::future::join_all(starts).await;
    }

    /// Stop every running agent, concurrently.
    pub async fn stop_all(&self) {
        let stops = self.slots().into_iter().map(|slot| async move {
            let _op = slot.op_lock.lock().await;
            // A slot deleted meanwhile has nothing left to stop.
            self.stop_locked(&slot).await.ok();
        });
        futures_util::future::join_all(stops).await;
    }

    /// Record the hub config that agents started from now on resolve against,
    /// and tell every running agent to reload against it.
    pub fn hub_config_changed(&self, hub_cfg: HubConfig) {
        *self.hub_cfg.write().unwrap_or_else(PoisonError::into_inner) = hub_cfg;
        self.signal_running_agents(ReloadSignal::Hub, "the hub config change");
    }

    /// Tell every running agent that files it reads from the shared team
    /// directory changed (its identity files, `AGENTS.md` and `USER.md`), so
    /// each reloads its workspace.
    pub fn team_files_changed(&self) {
        self.signal_running_agents(ReloadSignal::Workspace, "the team file change");
    }

    /// Send `signal` to every running agent; `about` names the cause in the
    /// log when an agent can't be reached.
    fn signal_running_agents(&self, signal: ReloadSignal, about: &str) {
        for slot in self.slots() {
            if let Some(running) = &slot.lock().running
                && running.control.reload_tx.send(signal).is_err()
            {
                tracing::warn!(agent = %slot.name, "couldn't tell a running agent about {about}: its reload channel is closed");
            }
        }
    }

    /// How many config reloads the running agent `name` has finished, or
    /// `None` when it isn't running.
    #[cfg(test)]
    pub(crate) fn reloads_finished(&self, name: &str) -> Option<u64> {
        let slot = self.slot(name).ok()?;
        let state = slot.lock();
        state
            .running
            .as_ref()
            .map(|running| *running.control.reload_done.borrow())
    }

    /// The live subagent sessions of every running agent.
    #[must_use]
    pub fn active_subagents(&self) -> Vec<crate::tracing_service::Subagent> {
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

    /// Publish the agent's summary as an `agent_state` event.
    fn publish_state(&self, slot: &AgentSlot) {
        self.publish(HubEvent::AgentState {
            agent: self.summary_of(slot),
        });
    }

    /// Move the agent to `state` and publish the change.
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

    /// Announce autostart or visibility changes made to the agent's config,
    /// whoever made them.
    fn refresh_meta(&self, slot: &AgentSlot) {
        let (_meta, changed) = slot.reload_meta();
        if changed {
            self.publish_state(slot);
        }
    }

    // ─── Start ────────────────────────────────────────────────────────

    /// Start the agent. The caller holds its `op_lock`.
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
            return Err(self
                .record_failure(
                    slot,
                    AgentErrorKind::PortConflict,
                    message,
                    &reason,
                    preview.as_ref(),
                )
                .await);
        }

        let inputs = AgentStartInputs {
            name: slot.name.clone(),
            agent_dir: slot.dir.clone(),
            services: self.services.clone(),
            hub_cfg,
            activity: Arc::clone(&slot.activity),
        };
        // Its own task, so a panic while starting is this agent's failure
        // instead of unwinding into the caller.
        let started = crate::util::spawn_in_span(
            async move { Box::pin(start_agent(inputs)).await }.instrument(agent_span(&slot.name)),
        )
        .await;
        match started {
            Ok(Ok(started)) => {
                let watcher = self.attach_watcher(slot, &started.control).await;
                self.attach(slot, started.runtime, started.control, watcher);
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
                Err(self
                    .record_failure(slot, kind, message, &err.to_string(), preview.as_ref())
                    .await)
            }
            Err(join_err) => {
                let detail = describe_join_error(join_err);
                let message = format!(
                    "{} hit an internal error while starting. Try starting it again; if it keeps happening, send a bug report.",
                    slot.name
                );
                Err(self
                    .record_failure(
                        slot,
                        AgentErrorKind::Crash,
                        message,
                        &detail,
                        preview.as_ref(),
                    )
                    .await)
            }
        }
    }

    /// Reserve Teams port `port` for `agent`, releasing any other port it
    /// held. Fails with the name of the agent that already holds the port.
    /// Checking and taking happen under one lock, so two agents starting at
    /// once can't both get it.
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

    /// Release the Teams port `agent` holds, if any.
    fn release_teams_port(&self, agent: &str) {
        let mut ports = self
            .teams_ports
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        ports.retain(|_, holder| holder != agent);
        self.publish_teams_ports(&ports);
    }

    /// The ports the tunnel dials for Teams, keyed by agent name.
    pub(crate) fn subscribe_teams_ports(&self) -> watch::Receiver<BTreeMap<String, u16>> {
        self.teams_ports_tx.subscribe()
    }

    /// Publish `ports` (keyed by port) as agent name to port.
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

    /// After a reload, move the agent's Teams port reservation to the port
    /// its new config uses. A port another agent holds is left to the
    /// adapter, whose failed bind is told to the user.
    fn refresh_teams_port(&self, slot: &AgentSlot) {
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

    /// Put the agent in `failed` with `message` and `kind`, log and
    /// auto-report `reason` (the underlying error, which the agent's
    /// last-error record also carries). The `agent_state` event this
    /// publishes is what the user hears of the failure. Returns the error a
    /// caller of `start` gets back.
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
                crate::tracing_service::client_context::gather_for_agent(&slot.name, cfg),
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

    /// Start watching a started agent, before its event loop runs so the
    /// watcher hears everything it does. An agent that can't be watched
    /// still runs; the hub just hears nothing from it.
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

    /// Record a successfully started agent: run its event loop as its own
    /// task under a supervisor, which stops `watcher` when the agent ends.
    fn attach(
        &self,
        slot: &Arc<AgentSlot>,
        runtime: crate::gateway::types::AgentRuntime,
        control: AgentControl,
        watcher: Option<AgentWatcher>,
    ) {
        let join = spawn_agent_loop(runtime);
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
        // The watcher is already listening, so whatever happened before now
        // is on disk or in the registry, and everything after reaches the
        // feed: a consumer recomputes the agent once, from here.
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
            crate::util::spawn_in_span(
                {
                    let host = Arc::clone(&host);
                    async move { host.supervise(supervised, join).await }
                }
                .instrument(agent_span(&slot.name)),
            );
            let slot = Arc::clone(slot);
            let mut reload_done = control.reload_done;
            crate::util::spawn_in_span(async move {
                while reload_done.changed().await.is_ok() {
                    host.refresh_meta(&slot);
                    host.refresh_teams_port(&slot);
                    host.spawn_team_embedding_refresh();
                }
            });
            self.spawn_team_embedding_refresh();
        }
    }

    // ─── Team wiki embedding ──────────────────────────────────────────

    /// Record the embedding model the team wiki index was opened with, so
    /// the first refresh doesn't reopen it for nothing.
    pub(crate) async fn note_team_embedding(&self, source: Option<EmbeddingSource>) {
        *self.team_embedding.lock().await = source;
    }

    fn spawn_team_embedding_refresh(&self) {
        if let Some(host) = self.me.upgrade() {
            crate::util::spawn_in_span(async move { host.refresh_team_embedding().await });
        }
    }

    /// Point the team wiki at the embedding model the agents now configure:
    /// the first agent (by name) with one. The one shared index swaps its
    /// embedder in place, so every agent's searcher sees the change. A choice
    /// that is unchanged, or that can't be made because an agent's config
    /// doesn't load right now, leaves the index alone.
    pub(crate) async fn refresh_team_embedding(&self) {
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

    /// Wait for the agent's event loop to end, then record how it ended.
    async fn supervise(&self, mut run: Supervised, join: tokio::task::JoinHandle<AgentExit>) {
        let exit = join.await;
        let requested = run.stop_requested.load(Ordering::SeqCst);
        // A failure is its message for the user and the underlying reason. A
        // run that ends on its own is always a crash.
        let failure: Option<(String, String)> = match exit {
            Ok(AgentExit::Stopped) => None,
            Ok(AgentExit::BusClosed) => Some((
                format!(
                    "{} stopped unexpectedly. Restart it from Home; if it keeps happening, send a bug report.",
                    run.slot.name
                ),
                "the event loop ended because its message channel closed".to_string(),
            )),
            // Only an abort after a timed-out stop is a stop; a panic while
            // shutting down is still a crash.
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
            // The event loop didn't shut the agent down itself.
            run.cleanup.run().await;
        }
        // After the agent's own shutdown, so what it published while winding
        // down (sessions recorded as interrupted) still reaches the feed.
        if let Some(watcher) = run.watcher.take() {
            watcher.stop().await;
        }
        run.slot.activity.run_ended();
        // Whether the agent shut down itself or died, it no longer takes part
        // in sibling discovery.
        self.services
            .sibling_fanout
            .unregister(&run.slot.name)
            .await;
        // Nor does it take teammate messages: nothing queues for a dead agent.
        self.services.team_router.unregister(&run.slot.name);

        let current = run.slot.lock().generation == run.generation;
        if current {
            match failure {
                None => self.set_state(&run.slot, AgentState::Stopped, None),
                Some((message, reason)) => {
                    let cfg = Config::load_agent_at(&run.slot.dir, &self.hub_config()).ok();
                    // Reported once, from here; `record_failure` does the rest.
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

    // ─── Stop ─────────────────────────────────────────────────────────

    /// Stop the agent and wait until it has wound down. The caller holds its
    /// `op_lock`. Does nothing for an agent that isn't running.
    ///
    /// # Errors
    /// Returns `NotFound` for an agent that has been deleted.
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
            return Ok(());
        };
        stop_requested.store(true, Ordering::SeqCst);
        // From the stop request on, no teammate message reaches the agent;
        // its event loop only unregisters once it sees the request.
        self.services.team_router.unregister(&slot.name);
        // Mirror that timing for the relay's A2A directory: it stops
        // advertising the agent now rather than waiting for the stop to
        // finish, which can take up to `STOP_TIMEOUT` plus `ABORT_SETTLE`.
        self.publish(HubEvent::AgentStopping {
            name: slot.name.clone(),
        });
        // A closed channel means the event loop is already gone.
        stop_tx.send(()).await.ok();
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

    // ─── Create and delete ────────────────────────────────────────────

    /// Create an agent from the blank template, start it, and hand it its
    /// role description as a first message.
    ///
    /// A creation whose start-up fails still creates the agent: it is
    /// returned in the `failed` state with its error, and the user can fix
    /// its settings and start it.
    async fn create_agent(
        &self,
        request: CreateAgentRequest,
        by: Actor,
    ) -> Result<AgentSummary, LifecycleError> {
        self.ensure_not_stopping()?;
        let display_name = crate::config::canonicalize_display_name(&request.name)
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
        // The agent exists either way; a failed start is its `failed` state.
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

    /// Log why a start that create or restore asked for did not happen. The
    /// agent stays in the state the summary reports (`failed` with its error,
    /// or `stopped` when the start was refused before it began).
    fn note_start_outcome(&self, slot: &AgentSlot, started: &Result<(), LifecycleError>) {
        if let Err(reason) = started {
            tracing::info!(
                agent = %slot.name,
                state = %self.summary_of(slot).state,
                reason = %reason,
                "agent was not started"
            );
        }
    }

    /// Deliver a new agent's role description to its main conversation, from
    /// its creator. A delivery failure is logged and told to the user; the
    /// agent is still created and can be given its role by hand.
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

    /// Stop the agent, checkpoint its directory, and remove the directory,
    /// its role page, and its roster entry.
    async fn delete_agent(&self, name: &str, by: Actor) -> Result<DeleteOutcome, LifecycleError> {
        let slot = self.slot(name)?;
        // Files, history, and the role page are keyed by the folder name,
        // even when `name` was the name people type.
        let slug = slot.name.clone();
        let display_name = self.summary_of(&slot).display_name;
        let checkpoint_id = {
            let _op = slot.op_lock.lock().await;
            self.stop_locked(&slot).await?;
            // Read before the role page goes, so a restore can bring it back.
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
            // Marked under the lock and before the slot is forgotten, so a
            // start or restart already holding this slot finds it gone
            // instead of starting the agent in a deleted directory.
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

    /// The folder a new agent called `display` should use.
    ///
    /// The same name, ignoring case, is refused when an agent already has it,
    /// and reused when a deleted agent had it so that agent's history
    /// continues. Otherwise the first free slug is used, skipping folders
    /// that still hold someone else's history.
    async fn choose_slug(&self, display: &str) -> Result<String, LifecycleError> {
        let key = crate::config::display_name_key(display);
        for slot in self.slots() {
            let meta = slot.current_meta();
            if crate::config::display_name_key(&meta.display_name) == key {
                return Err(LifecycleError::AlreadyExists(meta.display_name));
            }
        }
        let checkpoints = self.checkpoints_dir();
        let history = crate::checkpoints::agents_with_history(&checkpoints)
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
            if crate::config::display_name_key(&shown) == key {
                return Ok(slug.clone());
            }
        }
        crate::config::allocate_slug(&crate::config::slug_base(display), |candidate| {
            taken.iter().any(|slug| slug == candidate)
        })
        .map_err(LifecycleError::Failed)
    }

    /// The folder of the deleted agent `requested` names, by folder name or
    /// by the name people saw.
    async fn resolve_deleted_name(&self, requested: &str) -> Result<String, LifecycleError> {
        let checkpoints = self.checkpoints_dir();
        let history = crate::checkpoints::agents_with_history(&checkpoints)
            .map_err(|e| history_unreadable(&e))?;
        if crate::config::validate_agent_name(requested).is_ok()
            && history.iter().any(|slug| slug == requested)
        {
            return Ok(requested.to_string());
        }
        let canonical = crate::config::canonicalize_display_name(requested)
            .map_err(LifecycleError::InvalidName)?;
        let key = crate::config::display_name_key(&canonical);
        for slug in &history {
            let shown = super::deleted::read_deletion(&checkpoints, slug)
                .await
                .and_then(|record| record.display_name)
                .unwrap_or_else(|| slug.clone());
            if crate::config::display_name_key(&shown) == key {
                return Ok(slug.clone());
            }
        }
        Err(LifecycleError::NoDeletedAgent(requested.to_string()))
    }

    /// Refuse a name the hub already holds. A slot outlives its directory
    /// until a delete has finished, so this also stops a create or restore
    /// from racing the delete of the same name. Called under `creation_lock`.
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

    /// Restore a deleted agent from its checkpoint history, adopt it, and
    /// start it when its settings say to.
    async fn restore_agent(
        &self,
        request: RestoreAgentRequest,
        by: Actor,
    ) -> Result<AgentSummary, LifecycleError> {
        self.ensure_not_stopping()?;
        // A live agent answers before the deleted-history lookup, so restoring
        // a name that is in use is a conflict rather than "nothing to restore".
        if let Ok(slot) = self.slot(&request.name) {
            return Err(LifecycleError::AlreadyExists(
                slot.current_meta().display_name.clone(),
            ));
        }
        let name = self.resolve_deleted_name(&request.name).await?;
        {
            // The same lock as create, so a restore and a create of one name
            // can't both write the directory.
            let _creating = self.creation_lock.lock().await;
            self.ensure_name_free(&name)?;
            let dir = crate::config::paths::agent_dir(&self.services.root, &name);
            if tokio::fs::try_exists(dir.join("config").join("config.toml"))
                .await
                .unwrap_or(false)
            {
                return Err(LifecycleError::AlreadyExists(name));
            }
            if !crate::checkpoints::agents_with_history(&self.checkpoints_dir())
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
                        .show_checkpoint(crate::checkpoints::RepoKind::Workspace, id.clone())
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
            // The agent is restored either way; a failed start is its
            // `failed` state, which the user sees and can fix.
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

    /// The deleted agents, newest deletion first.
    async fn deleted_agents(&self) -> Result<Vec<DeletedAgent>, LifecycleError> {
        let checkpoints_dir = self.checkpoints_dir();
        let names = crate::checkpoints::agents_with_history(&checkpoints_dir)
            .map_err(|e| history_unreadable(&e))?;
        let mut deleted = Vec::new();
        for name in names {
            if crate::config::validate_agent_name(&name).is_err() {
                continue;
            }
            let dir = crate::config::paths::agent_dir(&self.services.root, &name);
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

    // ─── Patch ────────────────────────────────────────────────────────

    /// Write the patched settings to the agent's `config.toml` (checkpointing
    /// it first) and, for a running agent, wait for it to reload.
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
        let patched = crate::config::patch::apply_patch(&existing, &diff, "config.toml")
            .map_err(LifecycleError::Failed)?;
        Config::validate_agent_toml(&patched, &slot.dir, &slot.name, &self.hub_config())
            .map_err(|e| {
                LifecycleError::Failed(format!(
                    "{}'s settings would not be valid with that change, so nothing was changed: {e}",
                    slot.name
                ))
            })?;

        self.checkpoint_config_before_write(slot).await;
        crate::util::fs::atomic_write(&config_path, &patched)
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

    /// Checkpoint the agent's config repository before a write, the way the
    /// settings API does. Never fails the write.
    async fn checkpoint_config_before_write(&self, slot: &AgentSlot) {
        match self.checkpoint_engine(slot) {
            Ok(engine) => {
                let _checkpoint_id = engine
                    .checkpoint_config_kind_id_before_write(
                        crate::checkpoints::RepoKind::AgentConfig,
                        crate::checkpoints::CheckpointContext::system(
                            crate::checkpoints::CheckpointTrigger::PreConfigWrite,
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

    /// A checkpoint engine over the agent's repositories, rebuilt from its
    /// name and paths.
    fn checkpoint_engine(
        &self,
        slot: &AgentSlot,
    ) -> Result<Arc<crate::checkpoints::CheckpointEngine>, crate::checkpoints::CheckpointError>
    {
        self.checkpoint_engine_for(&slot.name, &slot.dir)
    }

    /// [`Self::checkpoint_engine`] for an agent that has no slot, such as a
    /// deleted one.
    fn checkpoint_engine_for(
        &self,
        name: &str,
        dir: &Path,
    ) -> Result<Arc<crate::checkpoints::CheckpointEngine>, crate::checkpoints::CheckpointError>
    {
        self.checkpoint_opener(name, dir)()
    }

    /// A function that opens the checkpoint engine over the agent's
    /// repositories each time it is called. It owns everything it needs, so
    /// a router can hold it past the borrow of `self`.
    fn checkpoint_opener(
        &self,
        name: &str,
        dir: &Path,
    ) -> impl Fn() -> Result<
        Arc<crate::checkpoints::CheckpointEngine>,
        crate::checkpoints::CheckpointError,
    > + Send
    + Sync
    + 'static {
        let shared = Arc::clone(&self.services.checkpoints);
        let team = self.services.team.clone();
        let checkpoints_dir = self.checkpoints_dir();
        let name = name.to_string();
        let dir = dir.to_path_buf();
        move || {
            crate::checkpoints::CheckpointEngine::with_shared_repos(
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

    fn checkpoints_dir(&self) -> PathBuf {
        crate::config::HubPaths::new(&self.services.hub_dir).checkpoints_dir()
    }

    // ─── Routers ──────────────────────────────────────────────────────

    /// The agent's config, file, and checkpoint routes, over its files on
    /// disk, so they work whether or not the agent is running.
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
        let state = crate::gateway::web::ConfigApiState {
            hub_dir: self.services.hub_dir.clone(),
            config_dir: slot.dir.join("config"),
            agent_name: slot.name.clone(),
            workspace_dir: slot.dir.clone(),
            memory_dir: Some(layout.memory_dir()),
            reload_tx,
            checkpoints,
            team: Some(self.services.team.view_for_user(&slot.dir)),
            scope: crate::gateway::web::WorkspaceScope::Agent,
        };
        Ok(crate::gateway::web::agent_repair_api_router(state))
    }

    /// The agent's chat history, usage, user inbox, and A2A settings routes,
    /// over its files on disk. Unlike [`Self::repair_router_for`] it opens no
    /// checkpoint repository: the one write that takes a checkpoint opens them
    /// when it runs, so these routes answer even when the repositories
    /// can't be opened.
    fn file_router_for(&self, slot: &AgentSlot) -> Router {
        let reload_tx = slot
            .lock()
            .running
            .as_ref()
            .map(|running| running.control.reload_tx.clone());
        crate::gateway::web::agent_files_api_router(crate::gateway::web::AgentFilesState {
            agent_name: slot.name.clone(),
            workspace_dir: slot.dir.clone(),
            memory_dir: Some(WorkspaceLayout::new(&slot.dir).memory_dir()),
            reload_tx,
            checkpoints: crate::gateway::web::CheckpointAccess::lazy(
                self.checkpoint_opener(&slot.name, &slot.dir),
            ),
        })
    }
}

/// What the supervisor task needs to record an agent's exit.
struct Supervised {
    slot: Arc<AgentSlot>,
    generation: u64,
    stop_requested: Arc<AtomicBool>,
    forced: Arc<AtomicBool>,
    done_tx: watch::Sender<bool>,
    cleanup: AgentCleanup,
    /// Stopped once the agent's run has ended.
    watcher: Option<AgentWatcher>,
}

/// A log-ready description of why an agent's task ended.
fn history_unreadable(error: &crate::checkpoints::CheckpointError) -> LifecycleError {
    tracing::error!(error = %error, "couldn't read the deleted agents' checkpoint history");
    LifecycleError::Failed(format!(
        "couldn't read the checkpoint history of deleted agents: {error}"
    ))
}

/// How many of a deleted agent's newest checkpoints are searched for the one
/// its deletion took when no record names it.
const DELETE_CHECKPOINT_SEARCH_DEPTH: usize = 50;

/// The workspace checkpoint a deleted agent is restored from by default: the
/// one its deletion recorded. Without a record (an agent deleted before
/// records were kept, or a record that couldn't be written) the newest
/// checkpoint the deletion itself took, else the newest one. The newest is
/// not always the deletion's: a turn-end checkpoint still in flight when the
/// agent stopped can land after it and see the directory already gone.
async fn pre_delete_checkpoint(
    engine: &crate::checkpoints::CheckpointEngine,
    name: &str,
    record: Option<&super::deleted::DeletionRecord>,
) -> Result<Option<crate::checkpoints::CheckpointSummary>, LifecycleError> {
    use crate::checkpoints::{CheckpointTrigger, RepoKind};
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
        format!("panicked: {}", crate::util::panic_message(&*payload))
    } else {
        format!("task ended abnormally: {error}")
    }
}

/// Who a lifecycle action counts as writing the team files it touches.
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
impl AgentDirectory for AgentHost {
    fn list(&self) -> Vec<AgentSummary> {
        self.slots()
            .iter()
            .map(|slot| self.summary_of(slot))
            .collect()
    }

    fn summary(&self, name: &str) -> Result<AgentSummary, LifecycleError> {
        Ok(self.summary_of(&*self.slot(name)?))
    }

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

    async fn create(
        &self,
        request: CreateAgentRequest,
        by: Actor,
    ) -> Result<AgentSummary, LifecycleError> {
        self.create_agent(request, by).await
    }

    async fn delete(&self, name: &str, by: Actor) -> Result<DeleteOutcome, LifecycleError> {
        let result = self.delete_agent(name, by.clone()).await;
        // An agent deleting itself is stopped by its own delete, so nothing is
        // left to hear the error; the user has to.
        if let (Err(e), Actor::Agent(actor)) = (&result, &by)
            && actor == name
        {
            tracing::error!(agent = %name, error = %e, "an agent's request to delete itself failed");
            self.notice(
                NoticeLevel::Warn,
                format!("{name} asked to be deleted, but that failed: {e}. It has been stopped; check its state on Home."),
                Some(name.to_string()),
            );
        }
        result
    }

    async fn list_deleted(&self) -> Result<Vec<DeletedAgent>, LifecycleError> {
        self.deleted_agents().await
    }

    async fn restore(
        &self,
        request: RestoreAgentRequest,
        by: Actor,
    ) -> Result<AgentSummary, LifecycleError> {
        self.restore_agent(request, by).await
    }

    async fn start(&self, name: &str) -> Result<AgentSummary, LifecycleError> {
        let slot = self.slot(name)?;
        let _op = slot.op_lock.lock().await;
        self.start_locked(&slot).await?;
        Ok(self.summary_of(&slot))
    }

    async fn stop(&self, name: &str) -> Result<AgentSummary, LifecycleError> {
        let slot = self.slot(name)?;
        let _op = slot.op_lock.lock().await;
        self.stop_locked(&slot).await?;
        Ok(self.summary_of(&slot))
    }

    async fn restart(&self, name: &str) -> Result<AgentSummary, LifecycleError> {
        self.ensure_not_stopping()?;
        let slot = self.slot(name)?;
        let _op = slot.op_lock.lock().await;
        self.stop_locked(&slot).await?;
        self.start_locked(&slot).await?;
        Ok(self.summary_of(&slot))
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

#[cfg(test)]
mod tests;
