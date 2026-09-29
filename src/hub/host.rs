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
//! the hub bus, in order, for the hub WebSocket to forward.

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
use super::directory::AgentDirectory;
use super::services::HubServices;
use super::types::{
    A2aVisibility, Actor, AgentActivity, AgentLastError, AgentPatch, AgentState, AgentSummary,
    CreateAgentRequest, DeleteOutcome, HubEvent, LifecycleError, NoticeLevel,
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

/// The two settings `patch` changes, as the agent's config file holds them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AgentMeta {
    autostart: bool,
    a2a_visibility: A2aVisibility,
}

impl AgentMeta {
    /// Read `autostart` and `[a2a] visibility` from the agent's
    /// `config/config.toml`, with the defaults an absent key resolves to. A
    /// file that can't be read or parsed reads as all defaults: the agent
    /// reports its own config problem when it starts.
    fn read(agent_dir: &Path) -> Self {
        let path = agent_dir.join("config").join("config.toml");
        let doc = match std::fs::read_to_string(&path) {
            Ok(text) => text.parse::<toml_edit::DocumentMut>().ok(),
            Err(e) => {
                tracing::debug!(error = %e, path = %path.display(), "couldn't read an agent's config for its summary");
                None
            }
        };
        let autostart = doc
            .as_ref()
            .and_then(|doc| doc.get("autostart"))
            .and_then(toml_edit::Item::as_bool)
            .unwrap_or(true);
        let a2a_visibility = match doc
            .as_ref()
            .and_then(|doc| doc.get("a2a"))
            .and_then(|a2a| a2a.get("visibility"))
            .and_then(toml_edit::Item::as_str)
            .map(str::trim)
        {
            Some("private") => A2aVisibility::Private,
            _ => A2aVisibility::Public,
        };
        Self {
            autostart,
            a2a_visibility,
        }
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
    /// The Teams adapter port this run holds, if it has one.
    teams_port: Option<u16>,
}

struct SlotState {
    state: AgentState,
    last_error: Option<AgentLastError>,
    running: Option<RunningAgent>,
    generation: u64,
    /// Autostart and visibility as last published, to notice a change made
    /// behind the host's back once the agent reloads.
    published_meta: AgentMeta,
}

/// One agent the host knows about.
struct AgentSlot {
    name: String,
    dir: PathBuf,
    /// Serializes start, stop, restart, and patch on this agent.
    op_lock: tokio::sync::Mutex<()>,
    state: Mutex<SlotState>,
    activity: Arc<ActivityTracker>,
}

impl AgentSlot {
    fn lock(&self) -> std::sync::MutexGuard<'_, SlotState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The hub's agents: lookup, per-agent routing, and lifecycle.
pub struct AgentHost {
    me: Weak<Self>,
    services: HubServices,
    hub_cfg: RwLock<HubConfig>,
    slots: RwLock<BTreeMap<String, Arc<AgentSlot>>>,
    events: broadcast::Sender<HubEvent>,
    /// Serializes creating agents, so two requests for one name can't both
    /// write its directory.
    creation_lock: tokio::sync::Mutex<()>,
}

impl AgentHost {
    /// A host over `services`, knowing no agents yet; call
    /// [`Self::discover`] to scan for them. `hub_cfg` is the hub config
    /// every agent resolves its own config against.
    pub(crate) fn new(services: HubServices, hub_cfg: HubConfig) -> Arc<Self> {
        let (events, _first_subscriber) = broadcast::channel(HUB_EVENT_CAPACITY);
        Arc::new_cyclic(|me| Self {
            me: Weak::clone(me),
            services,
            hub_cfg: RwLock::new(hub_cfg),
            slots: RwLock::new(BTreeMap::new()),
            events,
            creation_lock: tokio::sync::Mutex::new(()),
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
        self.slots
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(name)
            .cloned()
            .ok_or_else(|| LifecycleError::NotFound(name.to_string()))
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
        let meta = AgentMeta::read(&dir);
        slots.insert(
            name.to_string(),
            Arc::new(AgentSlot {
                name: name.to_string(),
                dir,
                op_lock: tokio::sync::Mutex::new(()),
                state: Mutex::new(SlotState {
                    state: AgentState::Stopped,
                    last_error: None,
                    running: None,
                    generation: 0,
                    published_meta: meta,
                }),
                activity: ActivityTracker::new(name, self.events.clone()),
            }),
        );
    }

    /// Forget the agent `name`, whose directory has been removed. The agent
    /// must already be stopped.
    pub fn forget(&self, name: &str) {
        self.slots
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(name);
    }

    /// Start every agent whose `autostart` is on, concurrently. A failure to
    /// start one is that agent's `failed` state, not an error here.
    pub async fn start_autostart(&self) {
        let starts = self
            .slots()
            .into_iter()
            .filter(|slot| AgentMeta::read(&slot.dir).autostart)
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
            self.stop_locked(&slot).await;
        });
        futures_util::future::join_all(stops).await;
    }

    /// Record the hub config that agents started from now on resolve against,
    /// and tell every running agent to reload against it.
    pub fn hub_config_changed(&self, hub_cfg: HubConfig) {
        *self.hub_cfg.write().unwrap_or_else(PoisonError::into_inner) = hub_cfg;
        for slot in self.slots() {
            if let Some(running) = &slot.lock().running
                && running.control.reload_tx.send(ReloadSignal::Hub).is_err()
            {
                tracing::warn!(agent = %slot.name, "couldn't tell a running agent about the hub config change: its reload channel is closed");
            }
        }
    }

    fn summary_of(&self, slot: &AgentSlot) -> AgentSummary {
        let (state, last_error) = {
            let guard = slot.lock();
            (guard.state, guard.last_error.clone())
        };
        let meta = AgentMeta::read(&slot.dir);
        AgentSummary {
            name: slot.name.clone(),
            state,
            last_error,
            autostart: meta.autostart,
            role: role_line(&self.team_paths(), &slot.name),
            a2a_visibility: meta.a2a_visibility,
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
            guard.published_meta = AgentMeta::read(&slot.dir);
        }
        self.publish_state(slot);
    }

    /// Announce autostart or visibility changes made to the agent's config,
    /// whoever made them.
    fn refresh_meta(&self, slot: &AgentSlot) {
        let current = AgentMeta::read(&slot.dir);
        let changed = {
            let mut guard = slot.lock();
            let changed = guard.published_meta != current;
            guard.published_meta = current;
            changed
        };
        if changed {
            self.publish_state(slot);
        }
    }

    // ─── Start ────────────────────────────────────────────────────────

    /// Start the agent. The caller holds its `op_lock`.
    async fn start_locked(&self, slot: &Arc<AgentSlot>) -> Result<(), LifecycleError> {
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
            && let Some(other) = self.teams_port_holder(&slot.name, port)
        {
            let message = format!(
                "{} can't start: its Teams adapter uses port {port}, which the agent '{other}' is already using. Give one of them a different Teams port, then start it again.",
                slot.name
            );
            return Err(self
                .record_failure(slot, message, "the Teams port is taken", preview.as_ref())
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
            async move { start_agent(inputs).await }.instrument(agent_span(&slot.name)),
        )
        .await;
        match started {
            Ok(Ok(started)) => {
                self.attach(slot, started.runtime, started.control, teams_port);
                Ok(())
            }
            Ok(Err(err)) => {
                let message = format!(
                    "{} couldn't start: {err}. Fix its settings or model configuration, then start it again.",
                    slot.name
                );
                Err(self
                    .record_failure(slot, message, &err.to_string(), preview.as_ref())
                    .await)
            }
            Err(join_err) => {
                let detail = describe_join_error(join_err);
                let message = format!(
                    "{} hit an internal error while starting. Try starting it again; if it keeps happening, send a bug report.",
                    slot.name
                );
                Err(self
                    .record_failure(slot, message, &detail, preview.as_ref())
                    .await)
            }
        }
    }

    /// The name of the running agent already holding Teams port `port`.
    fn teams_port_holder(&self, starting: &str, port: u16) -> Option<String> {
        self.slots()
            .into_iter()
            .filter(|slot| slot.name != starting)
            .find(|slot| {
                slot.lock()
                    .running
                    .as_ref()
                    .is_some_and(|running| running.teams_port == Some(port))
            })
            .map(|slot| slot.name.clone())
    }

    /// Put the agent in `failed` with `message`, log and auto-report
    /// `detail`, and leave the user an inbox item. Returns the error a
    /// caller of `start` gets back.
    async fn record_failure(
        &self,
        slot: &Arc<AgentSlot>,
        message: String,
        detail: &str,
        cfg: Option<&Config>,
    ) -> LifecycleError {
        tracing::error!(agent = %slot.name, error = %detail, "agent failed");
        self.services
            .tracing_service
            .on_error(
                &format!("agent failed: {detail}"),
                crate::tracing_service::client_context::gather_for_agent(&slot.name, cfg),
            )
            .await;
        self.set_state(
            slot,
            AgentState::Failed,
            Some(AgentLastError {
                message: message.clone(),
                at: Utc::now(),
            }),
        );
        self.leave_failure_in_inbox(slot, &message).await;
        LifecycleError::Failed(message)
    }

    /// Leave the failed agent's user inbox an item saying what happened, so
    /// the failure is there for the user even without the web UI open.
    async fn leave_failure_in_inbox(&self, slot: &AgentSlot, message: &str) {
        let layout = WorkspaceLayout::new(&slot.dir);
        let tz = self.hub_config().timezone;
        if let Err(e) = crate::inbox::quick_add(
            &layout.user_inbox_dir(),
            &format!("{} failed", slot.name),
            message,
            "hub",
            tz,
        )
        .await
        {
            tracing::warn!(agent = %slot.name, error = %e, "couldn't leave the agent's failure in its inbox");
        }
    }

    /// Record a successfully started agent: run its event loop as its own
    /// task under a supervisor.
    fn attach(
        &self,
        slot: &Arc<AgentSlot>,
        runtime: crate::gateway::types::AgentRuntime,
        control: AgentControl,
        teams_port: Option<u16>,
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
                teams_port,
            });
            guard.generation
        };
        self.set_state(slot, AgentState::Running, None);

        if let Some(host) = self.me.upgrade() {
            let supervised = Supervised {
                slot: Arc::clone(slot),
                generation,
                stop_requested,
                forced,
                done_tx,
                cleanup: control.cleanup.clone(),
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
                }
            });
        }
    }

    /// Wait for the agent's event loop to end, then record how it ended.
    async fn supervise(&self, run: Supervised, join: tokio::task::JoinHandle<AgentExit>) {
        let exit = join.await;
        let requested = run.stop_requested.load(Ordering::SeqCst);
        let failure: Option<(String, String)> = match exit {
            Ok(AgentExit::Stopped) => None,
            Ok(AgentExit::BusClosed) => Some((
                format!(
                    "{} stopped unexpectedly. Restart it from the team view; if it keeps happening, send a bug report.",
                    run.slot.name
                ),
                "the event loop ended because its message channel closed".to_string(),
            )),
            Err(_) if requested => None,
            Err(join_err) => Some((
                format!(
                    "{} crashed from an internal error. Restart it from the team view; if it keeps happening, send a bug report.",
                    run.slot.name
                ),
                describe_join_error(join_err),
            )),
        };
        if failure.is_some() || run.forced.load(Ordering::SeqCst) {
            // The event loop didn't shut the agent down itself.
            run.cleanup.run().await;
        }
        run.slot.activity.run_ended();

        let current = run.slot.lock().generation == run.generation;
        if current {
            match failure {
                None => self.set_state(&run.slot, AgentState::Stopped, None),
                Some((message, detail)) => {
                    let cfg = Config::load_agent_at(&run.slot.dir, &self.hub_config()).ok();
                    // Reported once, from here; `record_failure` does the rest.
                    self.record_failure(&run.slot, message, &detail, cfg.as_ref())
                        .await;
                }
            }
        }
        run.done_tx.send(true).ok();
    }

    // ─── Stop ─────────────────────────────────────────────────────────

    /// Stop the agent and wait until it has wound down. The caller holds its
    /// `op_lock`. Does nothing for an agent that isn't running.
    async fn stop_locked(&self, slot: &Arc<AgentSlot>) {
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
            return;
        };
        stop_requested.store(true, Ordering::SeqCst);
        // A closed channel means the event loop is already gone.
        stop_tx.send(()).await.ok();
        if tokio::time::timeout(STOP_TIMEOUT, done.wait_for(|finished| *finished))
            .await
            .is_ok()
        {
            return;
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
        let providers_toml = match (&request.models_from, &request.providers_toml) {
            (Some(from), _) => super::provision::copy_providers_from(&self.services.root, from)?,
            (None, Some(raw)) => raw.clone(),
            (None, None) => {
                return Err(LifecycleError::InvalidRequest(
                    "a new agent needs model settings: name an existing agent in models_from, or include providers_toml".to_string(),
                ));
            }
        };
        let spec = super::provision::AgentSpec {
            name: request.name.clone(),
            providers_toml,
            a2a_visibility: request.a2a_visibility.unwrap_or(A2aVisibility::Private),
            description: request.description.clone(),
        };
        {
            let _creating = self.creation_lock.lock().await;
            super::provision::provision_agent(
                &self.services.root,
                &self.team_paths(),
                &self.services.team,
                &team_writer(&by),
                &spec,
            )
            .await?;
            self.adopt(&request.name);
        }

        let slot = self.slot(&request.name)?;
        let started = {
            let _op = slot.op_lock.lock().await;
            self.start_locked(&slot).await
        };
        // The agent exists either way; a failed start is its `failed` state.
        if started.is_ok() {
            self.hand_over_role(&slot, &by, request.description.as_deref())
                .await;
        }
        let summary = self.summary_of(&slot);
        self.tell_acting_agent(
            &by,
            &format!("Created the agent {}", request.name),
            &format!(
                "You created the agent '{}'. It is {}.",
                request.name, summary.state
            ),
        )
        .await;
        self.publish(HubEvent::AgentCreated {
            agent: summary.clone(),
            by,
        });
        Ok(summary)
    }

    /// Leave a user inbox item for the agent that created or deleted another,
    /// so the outcome is there for the user beside that agent's work. The
    /// user acting through the web UI or CLI sees the toast only.
    async fn tell_acting_agent(&self, by: &Actor, title: &str, body: &str) {
        let Actor::Agent(actor) = by else {
            return;
        };
        let dir = crate::config::paths::agent_dir(&self.services.root, actor);
        let layout = WorkspaceLayout::new(&dir);
        if let Err(e) = crate::inbox::quick_add(
            &layout.user_inbox_dir(),
            title,
            body,
            "hub",
            self.hub_config().timezone,
        )
        .await
        {
            tracing::warn!(agent = %actor, error = %e, "couldn't leave a hub notice in the agent's inbox");
        }
    }

    /// Deliver a new agent's role description to its main conversation, from
    /// its creator. A delivery failure is logged and told to the user; the
    /// agent is still created and can be given its role by hand.
    async fn hand_over_role(&self, slot: &AgentSlot, by: &Actor, description: Option<&str>) {
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
            .deliver_to_main(by, message, self.hub_config().timezone)
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
        let checkpoint_id = {
            let _op = slot.op_lock.lock().await;
            self.stop_locked(&slot).await;
            let engine = self.checkpoint_engine(&slot).map_err(|e| {
                tracing::error!(agent = %name, error = %e, "couldn't open the agent's checkpoint repositories to delete it");
                LifecycleError::Failed(format!(
                    "couldn't open {name}'s checkpoint history, so it was not deleted: {e}"
                ))
            })?;
            super::provision::deprovision_agent(
                &self.services.root,
                &self.team_paths(),
                &self.services.team,
                &team_writer(&by),
                name,
                &engine,
            )
            .await?
        };
        self.forget(name);
        self.tell_acting_agent(
            &by,
            &format!("Deleted the agent {name}"),
            &format!("You deleted the agent '{name}'. Its files were checkpointed first, so it can be restored."),
        )
        .await;
        self.publish(HubEvent::AgentDeleted {
            name: name.to_string(),
            by,
        });
        Ok(DeleteOutcome {
            deleted: true,
            checkpoint_id,
        })
    }

    // ─── Patch ────────────────────────────────────────────────────────

    /// Write the patched settings to the agent's `config.toml` (checkpointing
    /// it first) and, for a running agent, wait for it to reload.
    async fn apply_patch(
        &self,
        slot: &Arc<AgentSlot>,
        patch: &AgentPatch,
    ) -> Result<(), LifecycleError> {
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
        let checkpoints_dir =
            crate::config::HubPaths::new(&self.services.hub_dir).checkpoints_dir();
        crate::checkpoints::CheckpointEngine::with_shared_repos(
            Arc::clone(&self.services.checkpoints),
            &slot.name,
            slot.dir.clone(),
            slot.dir.join("config"),
            &checkpoints_dir,
            None,
        )
        .map(|engine| Arc::new(engine.with_team_coordinator(self.services.team.clone())))
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
            setup_done: None,
            secret_lock: Arc::clone(&self.services.secret_lock),
            checkpoints,
            team: Some(self.services.team.view_for_agent(&slot.name, &slot.dir)),
        };
        Ok(crate::gateway::web::repair_router(state))
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
}

/// A log-ready description of why an agent's task ended.
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

    async fn create(
        &self,
        request: CreateAgentRequest,
        by: Actor,
    ) -> Result<AgentSummary, LifecycleError> {
        self.create_agent(request, by).await
    }

    async fn delete(&self, name: &str, by: Actor) -> Result<DeleteOutcome, LifecycleError> {
        self.delete_agent(name, by).await
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
        self.stop_locked(&slot).await;
        Ok(self.summary_of(&slot))
    }

    async fn restart(&self, name: &str) -> Result<AgentSummary, LifecycleError> {
        let slot = self.slot(name)?;
        let _op = slot.op_lock.lock().await;
        self.stop_locked(&slot).await;
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
