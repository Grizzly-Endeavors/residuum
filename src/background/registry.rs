//! Session registry: the single source of truth for every live agent session.
//!
//! Tracks address, run id, category, source label, lifecycle state, spawner,
//! depth, and purpose for every session that is currently running or idle.
//! Discovery tools (`list_agents`, `stop_agent`) and the bug-report client
//! context read this registry rather than any lower-level task map.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::Context as _;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::{Notify, mpsc};
use tokio_util::sync::CancellationToken;
use ts_rs::TS;

use crate::agent::interrupt::Interrupt;
use crate::bus::{ConversationTarget, EventTrigger, SessionAddress, SkillName};
use crate::config::BackgroundModelTier;

/// Capacity of a session's interrupt channel: agent messages delivered to it
/// (mid-turn, or to wake it while idle). Sized like the main agent's own
/// interrupt channel — deliveries are infrequent relative to this, so the
/// buffer exists to smooth bursts, not as a throughput bound.
pub(crate) const INTERRUPT_CHANNEL_CAPACITY: usize = 32;

/// The well-known address of the main agent. Never present in the registry
/// (main is not a session), but used as the `spawner` value for sessions
/// main creates directly.
pub const MAIN_ADDRESS: &str = "main";

/// The main agent's depth in the spawn tree.
pub const MAIN_DEPTH: u32 = 0;

/// Sender address for a message the owner types into a session from the web
/// UI's sessions sidebar. Not a session and never present in the registry:
/// the owner is not an agent, so a message to this address does not resolve.
pub const OWNER_ADDRESS: &str = "owner";

/// Prefix of the sender address a workbench artifact's message to a session
/// carries (`artifact:<name>`), and of an artifact session's source label.
/// Never present in the registry: an artifact is not an agent, so a message
/// to this address does not resolve. Session addresses never contain `:`,
/// so no session can be mistaken for an artifact.
pub const ARTIFACT_SENDER_PREFIX: &str = "artifact:";

/// Category label an artifact's message to a session carries as its
/// sender's category.
pub const ARTIFACT_SENDER_CATEGORY: &str = "artifact";

/// The sender address (and source label) naming the workbench artifact
/// `name`.
#[must_use]
pub fn artifact_sender_address(name: &str) -> SessionAddress {
    SessionAddress::from(format!("{ARTIFACT_SENDER_PREFIX}{name}"))
}

/// How a session was started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SessionCategory {
    /// Pulses and scheduled actions.
    Scheduled,
    /// Non-owner-DM conversations and webhooks.
    External,
    /// Started by an agent: `subagent_spawn` or the subconscious learner.
    Spawned,
    /// Started by a workbench artifact through the sessions HTTP API. Its
    /// output stays with the artifact: never relayed to main, never routed
    /// to the inbox or notification channels.
    Artifact,
}

impl SessionCategory {
    /// Derive the category from the trigger that started the session.
    #[must_use]
    pub fn from_trigger(trigger: &EventTrigger) -> Self {
        match trigger {
            EventTrigger::Pulse | EventTrigger::Action => Self::Scheduled,
            EventTrigger::Agent => Self::Spawned,
            EventTrigger::Webhook(_) | EventTrigger::Conversation => Self::External,
            EventTrigger::Artifact(_) => Self::Artifact,
        }
    }

    /// Lowercase label used in addresses, logs, and the store.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Scheduled => "scheduled",
            Self::External => "external",
            Self::Spawned => "spawned",
            Self::Artifact => "artifact",
        }
    }

    /// Parse the label [`Self::as_str`] produces, as recorded in the session
    /// store. `None` for anything else.
    #[must_use]
    pub fn from_label(label: &str) -> Option<Self> {
        match label {
            "scheduled" => Some(Self::Scheduled),
            "external" => Some(Self::External),
            "spawned" => Some(Self::Spawned),
            "artifact" => Some(Self::Artifact),
            _ => None,
        }
    }
}

impl std::fmt::Display for SessionCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A session's lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SessionState {
    /// The session is being forked (resources are being built); no turn has
    /// started yet.
    Forking,
    /// A turn is executing. Holds one concurrency permit.
    Running,
    /// The turn ended; the session is still live and will complete after its
    /// idle timeout, or resume immediately on new input.
    Idle,
    /// The idle timeout elapsed, or the session was stopped; memory is being
    /// finalized.
    Completing,
    /// Merged and closed. Immutable from here on.
    Completed,
}

impl SessionState {
    /// Lowercase label used in logs and the store.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Forking => "forking",
            Self::Running => "running",
            Self::Idle => "idle",
            Self::Completing => "completing",
            Self::Completed => "completed",
        }
    }

    /// Parse the label [`Self::as_str`] produces, as recorded in the session
    /// store. `None` for anything else.
    #[must_use]
    pub fn from_label(label: &str) -> Option<Self> {
        match label {
            "forking" => Some(Self::Forking),
            "running" => Some(Self::Running),
            "idle" => Some(Self::Idle),
            "completing" => Some(Self::Completing),
            "completed" => Some(Self::Completed),
            _ => None,
        }
    }
}

impl std::fmt::Display for SessionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What happened when [`SessionRegistry::deliver`] tried to hand a message
/// to a session's interrupt channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliverOutcome {
    /// Handed to the session's live interrupt channel.
    Delivered,
    /// The session exists but its current run is `completing`: it no longer
    /// accepts messages. The caller should resume it as a new run once this
    /// run has left the registry (see [`SessionRegistry::wait_until_clear`]).
    Completing,
    /// The session is live and accepting messages, but its interrupt
    /// channel is full. Vanishingly unlikely (32-deep, drained continuously
    /// by a live run), but distinct from [`Self::NotLive`] on purpose: a
    /// full channel must never be treated as "no such session" and fall
    /// through to a resume, which would register a second run at the same
    /// address.
    Full,
    /// No live session at this address.
    NotLive,
}

/// [`SessionRegistry::register`] refused because `address` already names a
/// live run.
#[derive(Debug, Clone)]
pub struct RegisterError {
    /// The address that was already live.
    pub address: SessionAddress,
}

impl std::fmt::Display for RegisterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "address {} is already registered to a live run",
            self.address
        )
    }
}

impl std::error::Error for RegisterError {}

/// A live session's metadata, as tracked by the registry.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    /// Stable address for this session.
    pub address: SessionAddress,
    /// Unique identifier for the current run within this session.
    pub run_id: String,
    /// How this session was started.
    pub category: SessionCategory,
    /// What triggered this session (precise origin, e.g. `Pulse`, `Webhook(name)`).
    pub trigger: EventTrigger,
    /// Human-readable source label (e.g. `"pulse:email_check"`).
    pub source_label: String,
    /// Current lifecycle state.
    pub state: SessionState,
    /// The agent that started this session (`main`, or another session's
    /// address). `None` for `scheduled` and `external` sessions.
    pub spawner: Option<SessionAddress>,
    /// Depth from the main agent (main = 0).
    pub depth: u32,
    /// One-line description of what this session is doing (the task brief,
    /// prompt, or conversation label, truncated).
    pub purpose: String,
    /// Skill the session runs with, if any.
    pub agent_skill: Option<SkillName>,
    /// Model tier this run executes at, carried onto a resume so a later run
    /// doesn't silently fall back to the default tier.
    pub model_tier: BackgroundModelTier,
    /// The conversation this session's turn output replies to, for an
    /// `external` session started by a conversation message. `None` for
    /// `scheduled`/`spawned` sessions and webhook-triggered `external` ones,
    /// which have no conversation of their own to reply into.
    pub conversation_target: Option<ConversationTarget>,
    /// When this run started.
    pub started_at: DateTime<Utc>,
    /// This run's cumulative token usage, updated after every model call —
    /// see [`SessionRegistry::accumulate_usage`]. Mirrored into the run's
    /// store record at completion so a finished run's totals survive too.
    pub usage: crate::agent::usage::SessionUsageTotals,
}

/// A registered session's bookkeeping: its metadata, the token that cancels
/// its in-flight turn or idle wait, and the sender half of its interrupt
/// channel — how a message addressed to this session actually reaches it
/// (an interrupt at its next tool-call boundary while running, or the input
/// for a new turn while idle; see `crate::background::messaging`).
struct SessionEntry {
    info: SessionInfo,
    stop_token: CancellationToken,
    interrupt_tx: mpsc::Sender<Interrupt>,
}

/// What's needed to resume a completed session as a new run at the same
/// address: enough of its last run's identity to fork it again, plus a
/// pointer to what that run produced.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResumePoint {
    /// Run id of the run that completed.
    pub previous_run_id: String,
    /// Episode the completed run was merged into, if any.
    pub previous_episode_id: Option<String>,
    /// What triggered the original run — determines the resumed run's
    /// category the same way it did the first time.
    pub trigger: EventTrigger,
    /// Human-readable source label carried over to the resumed run.
    pub source_label: String,
    /// Skill the original run executed with, if any, carried over so the
    /// resumed run keeps the same role.
    pub agent_skill: Option<SkillName>,
    /// Model tier the original run executed at, carried over so the resumed
    /// run doesn't default back to `Medium`.
    pub model_tier: BackgroundModelTier,
    /// The agent that spawned the original run, if any, carried over so the
    /// resumed run keeps the same spawner rather than losing it.
    pub spawner: Option<SessionAddress>,
    /// Depth from the main agent the original run had, carried over so the
    /// resumed run doesn't reset to depth 1 and evade the nesting cap.
    pub depth: u32,
    /// The conversation the original run replied to, carried over so a
    /// resumed conversation session still knows where to send its output.
    pub conversation_target: Option<ConversationTarget>,
    /// When this resume point was recorded, set by
    /// [`SessionRegistry::record_resume_point`] regardless of what a caller
    /// passes in — the timestamp a resume point's persisted copy is pruned
    /// against on load (see [`Self::MAX_AGE_DAYS`]).
    pub recorded_at: DateTime<Utc>,
}

impl ResumePoint {
    /// Age past which a persisted resume point is pruned on load, rather
    /// than kept forever for an address nobody has messaged in months.
    const MAX_AGE_DAYS: i64 = 90;
}

/// Registry of every live (running, idle, or completing) session, plus a
/// standing record of what's needed to resume a completed one.
///
/// Live sessions are removed once completed; their resume points are not —
/// an address must keep working for messaging across idle gaps and after
/// completion, for as long as the process runs, and — for a registry built
/// with [`Self::load`] — across a restart too, since resume points are
/// persisted write-through to disk as they're recorded.
#[derive(Default)]
pub struct SessionRegistry {
    sessions: Mutex<HashMap<SessionAddress, SessionEntry>>,
    resume_points: Mutex<HashMap<SessionAddress, ResumePoint>>,
    /// Where resume points are persisted write-through as they're recorded.
    /// `None` for a registry built with [`Self::new`] — resume points then
    /// live only in memory, as every unit test wants.
    persist_path: Option<PathBuf>,
    /// Held across snapshot-and-write so concurrent recordings reach disk in
    /// the order they were recorded; without it an older snapshot could land
    /// after a newer one and drop the newer resume point on restart.
    persist_lock: tokio::sync::Mutex<()>,
    /// Notified whenever any session is removed, so [`Self::wait_until_clear`]
    /// can wake without polling. A single registry-wide `Notify` rather than
    /// one per address: removals are infrequent, and a waiter re-checks its
    /// own address after waking, so a notification meant for a different
    /// address just costs a cheap recheck.
    cleared: Notify,
}

impl SessionRegistry {
    /// Create an empty registry with no persistence: resume points live only
    /// in memory and are lost on restart. Used by every unit test, and by
    /// any caller that doesn't need resume points to survive a process exit.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a registry whose resume points are persisted write-through to
    /// `persist_path`, loading whatever is already there.
    ///
    /// A missing file starts with no resume points, the same as
    /// [`Self::new`]. A file that exists but fails to read or parse is
    /// logged at `warn` with the path and error and likewise starts empty —
    /// a corrupt or unreadable resume-points file must never block startup,
    /// since the worst case is only that in-flight resumes fall back to
    /// fresh sessions. Entries older than [`ResumePoint::MAX_AGE_DAYS`] are
    /// dropped on load rather than kept (and immediately eligible for
    /// pruning) forever.
    #[must_use]
    pub async fn load(persist_path: PathBuf) -> Self {
        let resume_points = load_resume_points(&persist_path).await;
        Self {
            resume_points: Mutex::new(resume_points),
            persist_path: Some(persist_path),
            ..Self::default()
        }
    }

    /// Register a new session run, refusing if the address already names a
    /// live run.
    ///
    /// This is a compare-and-swap: the liveness check and the insert happen
    /// under the same lock, so two concurrent registration attempts for the
    /// same address can never both succeed and one silently clobber the
    /// other's live entry (its interrupt channel, its stop token) out from
    /// under it. A legitimate resume only ever calls this after
    /// [`Self::wait_until_clear`] confirms the address is empty, so refusal
    /// here means a second attempt genuinely raced in in the meantime — the
    /// loser must fall back to delivering its own input into whichever run
    /// won (see `SessionRuntime::spawn` and the spawn listener's liveness
    /// guard, both of which do exactly that).
    ///
    /// Returns the receiver half of the session's interrupt channel, for the
    /// runtime to drive the run's turns with.
    ///
    /// # Errors
    /// Returns [`RegisterError`] if the address is already registered.
    pub fn register(
        &self,
        info: SessionInfo,
        stop_token: CancellationToken,
    ) -> Result<mpsc::Receiver<Interrupt>, RegisterError> {
        let (interrupt_tx, interrupt_rx) = mpsc::channel(INTERRUPT_CHANNEL_CAPACITY);
        let address = info.address.clone();
        let mut guard = self.lock();
        if guard.contains_key(&address) {
            return Err(RegisterError { address });
        }
        guard.insert(
            address,
            SessionEntry {
                info,
                stop_token,
                interrupt_tx,
            },
        );
        Ok(interrupt_rx)
    }

    /// Deliver a message to a live session by address: an interrupt at its
    /// next tool-call boundary while running, or the input for a new turn
    /// while idle — the runtime on the other end of the channel decides
    /// which, depending on whether it currently owns the receiver inside an
    /// active turn or in its idle wait.
    ///
    /// See [`DeliverOutcome`] for what each outcome means to the caller.
    /// A `completing` run is deliberately never handed a message: its
    /// channel is about to be drained and dropped by its own teardown (see
    /// `crate::background::runtime::finish_run`), so anything accepted here
    /// after that point could be silently lost.
    pub fn deliver(&self, address: &SessionAddress, message: Interrupt) -> DeliverOutcome {
        let guard = self.lock();
        let Some(entry) = guard.get(address) else {
            return DeliverOutcome::NotLive;
        };
        if matches!(
            entry.info.state,
            SessionState::Completing | SessionState::Completed
        ) {
            return DeliverOutcome::Completing;
        }
        match entry.interrupt_tx.try_send(message) {
            Ok(()) => DeliverOutcome::Delivered,
            Err(e) => {
                tracing::warn!(address = %address, error = %e, "session interrupt channel full, refusing to deliver agent message");
                DeliverOutcome::Full
            }
        }
    }

    /// Wait until `address` is no longer registered, in any state.
    ///
    /// Used before starting a new run at an address whose previous run is
    /// still tearing down (`completing`), so the two can never coexist.
    /// Unbounded: a `completing` run is always guaranteed to eventually
    /// leave the registry (`finish_run` and panic recovery both end on
    /// `remove`), so there is no address this could wait on forever absent a
    /// bug elsewhere.
    ///
    /// Driven by [`Self::cleared`] rather than polling: the `Notify` future
    /// is obtained *before* the address is checked, per `tokio::sync::Notify`'s
    /// documented check-then-wait pattern, so a removal that races between
    /// the check and the `.await` is never missed. A notification meant for a
    /// different address just costs a cheap recheck and another wait. Logs
    /// once if the wait runs unusually long, since that would mean the
    /// previous run's teardown is stuck.
    pub async fn wait_until_clear(&self, address: &SessionAddress) {
        const WARN_AFTER: Duration = Duration::from_secs(30);

        let start = std::time::Instant::now();
        let mut warned = false;
        loop {
            let notified = self.cleared.notified();
            if self.get(address).is_none() {
                return;
            }
            if tokio::time::timeout(WARN_AFTER, notified).await.is_err() && !warned {
                tracing::warn!(
                    address = %address,
                    waited_secs = start.elapsed().as_secs(),
                    "still waiting for a completing session to leave the registry"
                );
                warned = true;
            }
        }
    }

    /// Record what's needed to resume a session as a new run once its
    /// current run completes. Overwrites any previous resume point at the
    /// same address, since only the most recent run's pointer matters.
    ///
    /// Stamps `point.recorded_at` with the current time regardless of what
    /// the caller passed, since that field exists only to let a later load
    /// prune stale entries, not to record when the underlying run actually
    /// completed. When this registry was built with [`Self::load`], the
    /// updated map is written through to disk before returning — a resume
    /// point that only exists in memory would be lost on the next restart,
    /// defeating the point of persisting it at all. A write failure is
    /// logged at `warn` with the path and error; the in-memory record still
    /// stands either way, so a message to this address still resumes it for
    /// as long as this process keeps running.
    pub async fn record_resume_point(&self, address: &SessionAddress, mut point: ResumePoint) {
        point.recorded_at = Utc::now();
        let _persist_guard = self.persist_lock.lock().await;
        let snapshot = {
            let mut guard = self
                .resume_points
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.insert(address.clone(), point);
            self.persist_path.as_ref().map(|_| guard.clone())
        };
        let (Some(path), Some(snapshot)) = (self.persist_path.as_ref(), snapshot) else {
            return;
        };
        if let Err(e) = persist_resume_points(path, &snapshot).await {
            tracing::warn!(path = %path.display(), error = %e, "failed to persist resume points to disk");
        }
    }

    /// Look up a completed session's resume point, if it has ever run.
    #[must_use]
    pub fn resume_point(&self, address: &SessionAddress) -> Option<ResumePoint> {
        self.resume_points
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(address)
            .cloned()
    }

    /// Update a session's lifecycle state. No-op if the address is unknown
    /// (the session may already have been removed).
    pub fn set_state(&self, address: &SessionAddress, state: SessionState) {
        let mut guard = self.lock();
        if let Some(entry) = guard.get_mut(address) {
            entry.info.state = state;
        }
    }

    /// Fold one model call's usage into a session's running totals.
    ///
    /// Returns the updated totals, or `None` if the address is unknown —
    /// the session may already have completed and left the registry,
    /// which the caller (the turn's [`crate::agent::usage::UsageSink`])
    /// treats the same as a provider reporting no usage.
    pub fn accumulate_usage(
        &self,
        address: &SessionAddress,
        usage: Option<crate::inference::Usage>,
    ) -> Option<crate::agent::usage::SessionUsageTotals> {
        let mut guard = self.lock();
        let entry = guard.get_mut(address)?;
        entry.info.usage.accumulate(usage);
        Some(entry.info.usage)
    }

    /// Stop a session: cancels its stop token so an in-flight turn or idle
    /// wait ends and the session moves to `completing`.
    ///
    /// Returns `true` if a live session was found and signalled, `false` if
    /// the address is unknown or the session is already completing/completed.
    pub fn stop(&self, address: &SessionAddress) -> bool {
        let guard = self.lock();
        let Some(entry) = guard.get(address) else {
            return false;
        };
        if matches!(
            entry.info.state,
            SessionState::Completing | SessionState::Completed
        ) {
            return false;
        }
        entry.stop_token.cancel();
        true
    }

    /// Stop `address`'s turn, but only if one is actually running right now.
    ///
    /// Unlike [`Self::stop`] (which also ends an idle or forking session
    /// outright, moving it straight to `completing`), this leaves an idle
    /// session alone: it's used where "stop" means "interrupt whatever this
    /// conversation is doing right now", such as a chat interface's `/stop`
    /// command, and an idle session isn't doing anything to interrupt. The
    /// check and the cancel happen under the same lock, so the decision is
    /// always made against the address's actual state at the instant this is
    /// called — there is no window where a request can be queued and later
    /// misapplied to a turn that starts afterward.
    ///
    /// Returns `true` if a running turn was found and signalled to stop,
    /// `false` if the address has no live session or its session isn't
    /// currently running a turn (idle, forking, completing, or completed) —
    /// in which case nothing is touched.
    pub fn stop_if_running(&self, address: &SessionAddress) -> bool {
        let guard = self.lock();
        let Some(entry) = guard.get(address) else {
            return false;
        };
        if entry.info.state != SessionState::Running {
            return false;
        }
        entry.stop_token.cancel();
        true
    }

    /// Stop every live session (running, idle, or forking) — used at gateway
    /// shutdown so runs complete and are recorded rather than being left for
    /// startup recovery on the next boot.
    ///
    /// Returns the number of sessions signalled. Sessions already
    /// `completing`/`completed` are left alone, matching [`stop`](Self::stop).
    pub fn stop_all(&self) -> usize {
        let guard = self.lock();
        let mut signalled = 0;
        for entry in guard.values() {
            if matches!(
                entry.info.state,
                SessionState::Completing | SessionState::Completed
            ) {
                continue;
            }
            entry.stop_token.cancel();
            signalled += 1;
        }
        signalled
    }

    /// Remove a session from the registry once it has fully completed, but
    /// only if the entry still registered there belongs to `run_id`.
    ///
    /// A stale run's teardown (a panic-recovery path, or a run that raced a
    /// newer one at the same address) must never delete a newer run's live
    /// entry — checking the run id before removing is what makes that
    /// impossible. Returns `None` (no-op) if the address is unknown or now
    /// holds a different run.
    pub fn remove(&self, address: &SessionAddress, run_id: &str) -> Option<SessionInfo> {
        let removed = {
            let mut guard = self.lock();
            if guard.get(address).is_none_or(|e| e.info.run_id != run_id) {
                return None;
            }
            guard.remove(address).map(|entry| entry.info)
        };
        // Notified after the lock is dropped, and unconditionally on every
        // removal (not just ones a waiter is known to care about) — cheap,
        // and it keeps this method from needing to track who's waiting.
        self.cleared.notify_waiters();
        removed
    }

    /// Look up a live session's current info by run id rather than address.
    #[must_use]
    pub fn get_by_run_id(&self, run_id: &str) -> Option<SessionInfo> {
        self.lock()
            .values()
            .find(|entry| entry.info.run_id == run_id)
            .map(|entry| entry.info.clone())
    }

    /// Look up a single session's current info.
    #[must_use]
    pub fn get(&self, address: &SessionAddress) -> Option<SessionInfo> {
        self.lock().get(address).map(|entry| entry.info.clone())
    }

    /// Snapshot of every live session, for `list_agents` and the web listing.
    #[must_use]
    pub fn list_live(&self) -> Vec<SessionInfo> {
        let guard = self.lock();
        let mut sessions: Vec<SessionInfo> = guard.values().map(|e| e.info.clone()).collect();
        sessions.sort_by_key(|s| s.started_at);
        sessions
    }

    /// Snapshot formatted for the bug-report `active_subagents` field.
    #[must_use]
    pub fn subagent_snapshot(&self) -> Vec<crate::tracing_service::Subagent> {
        self.list_live()
            .into_iter()
            .map(|info| crate::tracing_service::Subagent {
                name: info.address.to_string(),
                status: format!("{} ({})", info.state, info.source_label),
            })
            .collect()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<SessionAddress, SessionEntry>> {
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// A session's own [`crate::agent::usage::UsageSink`]: accumulates onto its
/// registry entry, which mirrors into the run's store record at
/// completion (see [`crate::background::store::RunRecord::starting`]) so
/// a finished run's totals aren't lost.
pub struct SessionUsageSink<'a> {
    /// The registry the session's entry lives in.
    pub registry: &'a SessionRegistry,
    /// The session's address.
    pub address: SessionAddress,
}

#[async_trait::async_trait]
impl crate::agent::usage::UsageSink for SessionUsageSink<'_> {
    async fn accumulate(
        &self,
        usage: Option<crate::inference::Usage>,
    ) -> crate::agent::usage::SessionUsageTotals {
        self.registry
            .accumulate_usage(&self.address, usage)
            .unwrap_or_default()
    }
}

/// Load persisted resume points from `path`, pruning entries older than
/// [`ResumePoint::MAX_AGE_DAYS`].
///
/// A missing file, or one that fails to read or parse, produces an empty map
/// rather than an error — see [`SessionRegistry::load`] for why a load
/// failure here must never block startup.
async fn load_resume_points(path: &Path) -> HashMap<SessionAddress, ResumePoint> {
    let contents = match tokio::fs::read_to_string(path).await {
        Ok(contents) => contents,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return HashMap::new(),
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "failed to read persisted resume points, starting with none"
            );
            return HashMap::new();
        }
    };
    if contents.trim().is_empty() {
        return HashMap::new();
    }
    let points: HashMap<SessionAddress, ResumePoint> = match serde_json::from_str(&contents) {
        Ok(points) => points,
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "failed to parse persisted resume points, starting with none"
            );
            return HashMap::new();
        }
    };
    let cutoff = Utc::now() - chrono::Duration::days(ResumePoint::MAX_AGE_DAYS);
    points
        .into_iter()
        .filter(|(_, point)| point.recorded_at >= cutoff)
        .collect()
}

/// Write `points` to `path` atomically (temp file plus rename), replacing
/// whatever was there.
///
/// # Errors
/// Returns an error if serialization or the underlying write fails.
async fn persist_resume_points(
    path: &Path,
    points: &HashMap<SessionAddress, ResumePoint>,
) -> anyhow::Result<()> {
    let json = serde_json::to_string_pretty(points).context("failed to serialize resume points")?;
    crate::util::fs::atomic_write(path, &json).await
}

/// Generate a unique run id, distinct from the session's address and unique
/// across all runs (including later runs of the same session).
#[must_use]
pub fn generate_run_id() -> String {
    let timestamp_ms = chrono::Utc::now().timestamp_millis();
    let rand_part: u32 = rand::random();
    format!("run-{timestamp_ms}-{rand_part:08x}")
}

/// Generate a human-readable, URL/filename-safe session address for a new
/// `scheduled` or `spawned` session, or a webhook's `external` session.
///
/// Prefixed by category and a slugified qualifier (e.g. a skill, pulse, or
/// webhook name), suffixed with a short random hex tag for uniqueness.
#[must_use]
pub fn generate_address(trigger: &EventTrigger, qualifier: &str) -> SessionAddress {
    let category = SessionCategory::from_trigger(trigger);
    let slug = slugify(qualifier);
    let suffix: u16 = rand::random();
    SessionAddress::from(format!("{category}-{slug}-{suffix:04x}"))
}

/// Deterministic, URL/filename-safe address for an external conversation
/// session, derived from its interface endpoint and its stable conversation
/// id.
///
/// Every message in the same conversation resolves to this same address,
/// whether or not a run is currently live there — and the address is stable
/// across restarts, since it depends only on its inputs, not on random state
/// or process-lifetime counters. Conversation ids vary wildly in shape
/// across interfaces (Teams ids like `19:abc@thread.tacv2` carry characters
/// that are unsafe in a URL path segment or a filename, especially on
/// Windows), so the id is hashed rather than embedded directly.
#[must_use]
pub fn conversation_session_address(endpoint: &str, conversation_id: &str) -> SessionAddress {
    let mut input = endpoint.as_bytes().to_vec();
    input.push(0);
    input.extend_from_slice(conversation_id.as_bytes());
    let hash = fnv1a64(&input);
    SessionAddress::from(format!("external-{}-{hash:016x}", slugify(endpoint)))
}

/// FNV-1a 64-bit hash. Not cryptographic — just a small, dependency-free,
/// deterministic hash for turning an arbitrary conversation id into a
/// fixed-width, filename-safe tag. Deterministic across processes, restarts,
/// and platforms, unlike `std`'s `DefaultHasher` (whose algorithm and output
/// are documented as unspecified and may change between Rust releases).
fn fnv1a64(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET_BASIS;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

/// Lowercase, hyphenate, and cap a free-text qualifier for use in an address.
fn slugify(s: &str) -> String {
    let mut slug: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .take(24)
        .collect();
    while slug.contains("--") {
        slug = slug.replace("--", "-");
    }
    let trimmed = slug.trim_matches('-');
    if trimmed.is_empty() {
        "session".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_info(address: &str) -> SessionInfo {
        SessionInfo {
            address: SessionAddress::from(address),
            run_id: "run-1".to_string(),
            category: SessionCategory::Spawned,
            trigger: EventTrigger::Agent,
            source_label: "agent:researcher".to_string(),
            state: SessionState::Running,
            spawner: Some(SessionAddress::from(MAIN_ADDRESS)),
            depth: 1,
            purpose: "research the thing".to_string(),
            agent_skill: None,
            model_tier: BackgroundModelTier::Medium,
            conversation_target: None,
            started_at: Utc::now(),
            usage: crate::agent::usage::SessionUsageTotals::default(),
        }
    }

    #[test]
    fn category_from_trigger() {
        assert_eq!(
            SessionCategory::from_trigger(&EventTrigger::Pulse),
            SessionCategory::Scheduled
        );
        assert_eq!(
            SessionCategory::from_trigger(&EventTrigger::Action),
            SessionCategory::Scheduled
        );
        assert_eq!(
            SessionCategory::from_trigger(&EventTrigger::Agent),
            SessionCategory::Spawned
        );
        assert_eq!(
            SessionCategory::from_trigger(&EventTrigger::Webhook("gh".into())),
            SessionCategory::External
        );
        assert_eq!(
            SessionCategory::from_trigger(&EventTrigger::Conversation),
            SessionCategory::External
        );
        assert_eq!(
            SessionCategory::from_trigger(&EventTrigger::Artifact("wiki".into())),
            SessionCategory::Artifact
        );
    }

    #[test]
    fn every_category_label_round_trips() {
        for category in [
            SessionCategory::Scheduled,
            SessionCategory::External,
            SessionCategory::Spawned,
            SessionCategory::Artifact,
        ] {
            assert_eq!(
                SessionCategory::from_label(category.as_str()),
                Some(category)
            );
        }
        assert_eq!(SessionCategory::from_label("mystery"), None);
    }

    #[test]
    fn artifact_session_address_is_prefixed_with_its_category() {
        let address = generate_address(&EventTrigger::Artifact("wiki".into()), "wiki graph");
        assert!(
            address.as_ref().starts_with("artifact-wiki-graph-"),
            "got {address}"
        );
        assert!(
            !address.as_ref().contains(':'),
            "session addresses never contain ':', so they can't collide with an artifact sender"
        );
    }

    #[test]
    fn conversation_session_address_is_stable_for_the_same_conversation() {
        let a = conversation_session_address("discord", "12345");
        let b = conversation_session_address("discord", "12345");
        assert_eq!(
            a, b,
            "the same conversation must always resolve to the same address"
        );
    }

    #[test]
    fn conversation_session_address_differs_by_conversation_id() {
        let a = conversation_session_address("discord", "12345");
        let b = conversation_session_address("discord", "67890");
        assert_ne!(a, b);
    }

    #[test]
    fn conversation_session_address_differs_by_endpoint() {
        let a = conversation_session_address("discord", "12345");
        let b = conversation_session_address("telegram", "12345");
        assert_ne!(
            a, b,
            "the same conversation id on two different interfaces must not collide"
        );
    }

    #[test]
    fn conversation_session_address_is_url_and_filename_safe() {
        // Teams conversation ids carry characters unsafe in a URL path
        // segment or a filename (colons, `@`), which is exactly why the id
        // is hashed rather than embedded.
        let addr = conversation_session_address("teams", "19:abc@thread.tacv2");
        let s = addr.as_ref();
        assert!(
            s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
            "address must contain only URL/filename-safe characters, got {s}"
        );
    }

    #[test]
    fn conversation_session_address_is_prefixed_by_endpoint() {
        let addr = conversation_session_address("discord", "12345");
        assert!(addr.as_ref().starts_with("external-discord-"));
    }

    #[test]
    fn register_and_get_round_trips() {
        let registry = SessionRegistry::new();
        let info = sample_info("spawned-researcher-0001");
        let _rx = registry
            .register(info.clone(), CancellationToken::new())
            .unwrap();

        let got = registry.get(&info.address).unwrap();
        assert_eq!(got.address, info.address);
        assert_eq!(got.state, SessionState::Running);
    }

    #[test]
    fn register_refuses_a_second_run_at_an_address_already_live() {
        let registry = SessionRegistry::new();
        let info = sample_info("spawned-researcher-cas0001");
        let _rx = registry
            .register(info.clone(), CancellationToken::new())
            .unwrap();

        let mut other = sample_info("spawned-researcher-cas0001");
        other.run_id = "run-2".to_string();
        let err = registry
            .register(other, CancellationToken::new())
            .expect_err("a second registration at a live address must be refused");
        assert_eq!(err.address, info.address);

        // The compare-and-swap must leave the original entry untouched.
        assert_eq!(
            registry.get(&info.address).unwrap().run_id,
            info.run_id,
            "a refused registration must not clobber the live entry it lost to"
        );
    }

    #[test]
    fn register_succeeds_again_once_the_address_has_been_removed() {
        let registry = SessionRegistry::new();
        let info = sample_info("spawned-researcher-cas0002");
        let _rx = registry
            .register(info.clone(), CancellationToken::new())
            .unwrap();
        registry.remove(&info.address, &info.run_id);

        let mut resumed = sample_info("spawned-researcher-cas0002");
        resumed.run_id = "run-resumed".to_string();
        assert!(
            registry.register(resumed, CancellationToken::new()).is_ok(),
            "a legitimate resume, registering after the old run cleared, must still succeed"
        );
    }

    #[test]
    fn set_state_updates_existing_session() {
        let registry = SessionRegistry::new();
        let info = sample_info("spawned-researcher-0002");
        let _rx = registry
            .register(info.clone(), CancellationToken::new())
            .unwrap();

        registry.set_state(&info.address, SessionState::Idle);
        assert_eq!(
            registry.get(&info.address).unwrap().state,
            SessionState::Idle
        );
    }

    #[test]
    fn set_state_on_unknown_address_is_a_noop() {
        let registry = SessionRegistry::new();
        registry.set_state(&SessionAddress::from("nope"), SessionState::Idle);
        assert!(registry.get(&SessionAddress::from("nope")).is_none());
    }

    fn usage(input: u32, output: u32) -> crate::inference::Usage {
        crate::inference::Usage {
            input_tokens: input,
            output_tokens: output,
            cache_creation_tokens: None,
            cache_read_tokens: None,
        }
    }

    #[test]
    fn accumulate_usage_folds_into_the_running_session_entry() {
        let registry = SessionRegistry::new();
        let info = sample_info("spawned-researcher-0010");
        let _rx = registry
            .register(info.clone(), CancellationToken::new())
            .unwrap();

        let first = registry
            .accumulate_usage(&info.address, Some(usage(100, 20)))
            .expect("a live session should accumulate");
        assert_eq!(first.input_tokens, 100);
        assert_eq!(first.output_tokens, 20);

        let second = registry
            .accumulate_usage(&info.address, Some(usage(50, 10)))
            .expect("a live session should still accumulate");
        assert_eq!(
            second.input_tokens, 150,
            "totals must accumulate across calls"
        );
        assert_eq!(second.output_tokens, 30);

        assert_eq!(
            registry.get(&info.address).unwrap().usage,
            second,
            "the registry entry's own usage field must reflect the latest totals"
        );
    }

    #[test]
    fn accumulate_usage_on_unknown_address_returns_none() {
        let registry = SessionRegistry::new();
        assert!(
            registry
                .accumulate_usage(&SessionAddress::from("ghost"), Some(usage(10, 5)))
                .is_none()
        );
    }

    #[tokio::test]
    async fn session_usage_sink_accumulates_through_the_registry() {
        let registry = SessionRegistry::new();
        let info = sample_info("spawned-researcher-0011");
        let _rx = registry
            .register(info.clone(), CancellationToken::new())
            .unwrap();

        let sink = SessionUsageSink {
            registry: &registry,
            address: info.address.clone(),
        };
        let totals = crate::agent::usage::UsageSink::accumulate(&sink, Some(usage(200, 40))).await;
        assert_eq!(totals.input_tokens, 200);
        assert_eq!(registry.get(&info.address).unwrap().usage.output_tokens, 40);
    }

    #[tokio::test]
    async fn session_usage_sink_for_a_completed_session_returns_default() {
        let registry = SessionRegistry::new();
        let sink = SessionUsageSink {
            registry: &registry,
            address: SessionAddress::from("gone"),
        };
        let totals = crate::agent::usage::UsageSink::accumulate(&sink, Some(usage(10, 5))).await;
        assert_eq!(
            totals,
            crate::agent::usage::SessionUsageTotals::default(),
            "a session no longer in the registry has nowhere to accumulate; the sink degrades to a no-op"
        );
    }

    #[test]
    fn stop_cancels_token_for_running_session() {
        let registry = SessionRegistry::new();
        let info = sample_info("spawned-researcher-0003");
        let token = CancellationToken::new();
        let _rx = registry.register(info.clone(), token.clone()).unwrap();

        assert!(registry.stop(&info.address));
        assert!(token.is_cancelled());
    }

    #[test]
    fn stop_returns_false_for_unknown_address() {
        let registry = SessionRegistry::new();
        assert!(!registry.stop(&SessionAddress::from("ghost")));
    }

    #[test]
    fn stop_returns_false_for_already_completing_session() {
        let registry = SessionRegistry::new();
        let mut info = sample_info("spawned-researcher-0004");
        info.state = SessionState::Completing;
        let _rx = registry
            .register(info.clone(), CancellationToken::new())
            .unwrap();

        assert!(!registry.stop(&info.address));
    }

    #[test]
    fn stop_if_running_cancels_token_for_a_running_session() {
        let registry = SessionRegistry::new();
        let info = sample_info("spawned-researcher-0006");
        let token = CancellationToken::new();
        let _rx = registry.register(info.clone(), token.clone()).unwrap();

        assert!(registry.stop_if_running(&info.address));
        assert!(token.is_cancelled());
    }

    #[test]
    fn stop_if_running_leaves_an_idle_session_untouched() {
        let registry = SessionRegistry::new();
        let mut info = sample_info("spawned-researcher-0007");
        info.state = SessionState::Idle;
        let token = CancellationToken::new();
        let _rx = registry.register(info.clone(), token.clone()).unwrap();

        assert!(
            !registry.stop_if_running(&info.address),
            "an idle session has nothing running to stop"
        );
        assert!(
            !token.is_cancelled(),
            "an idle session must not be cancelled by stop_if_running"
        );
        assert_eq!(
            registry.get(&info.address).unwrap().state,
            SessionState::Idle
        );
    }

    #[test]
    fn stop_if_running_returns_false_for_unknown_address() {
        let registry = SessionRegistry::new();
        assert!(!registry.stop_if_running(&SessionAddress::from("ghost")));
    }

    #[test]
    fn stop_if_running_returns_false_for_a_forking_session() {
        let registry = SessionRegistry::new();
        let mut info = sample_info("spawned-researcher-0008");
        info.state = SessionState::Forking;
        let token = CancellationToken::new();
        let _rx = registry.register(info.clone(), token.clone()).unwrap();

        assert!(!registry.stop_if_running(&info.address));
        assert!(!token.is_cancelled());
    }

    #[test]
    fn stop_all_cancels_every_live_session_but_skips_completing() {
        let registry = SessionRegistry::new();
        let running = sample_info("spawned-a-0001");
        let mut idle = sample_info("spawned-b-0002");
        idle.state = SessionState::Idle;
        let mut completing = sample_info("spawned-c-0003");
        completing.state = SessionState::Completing;

        let running_token = CancellationToken::new();
        let idle_token = CancellationToken::new();
        let completing_token = CancellationToken::new();
        let _running_rx = registry
            .register(running.clone(), running_token.clone())
            .unwrap();
        let _idle_rx = registry.register(idle.clone(), idle_token.clone()).unwrap();
        let _completing_rx = registry
            .register(completing.clone(), completing_token.clone())
            .unwrap();

        let signalled = registry.stop_all();

        assert_eq!(
            signalled, 2,
            "only the running and idle sessions should be signalled"
        );
        assert!(running_token.is_cancelled());
        assert!(idle_token.is_cancelled());
        assert!(
            !completing_token.is_cancelled(),
            "a session already completing should be left alone"
        );
    }

    #[test]
    fn stop_all_on_empty_registry_returns_zero() {
        let registry = SessionRegistry::new();
        assert_eq!(registry.stop_all(), 0);
    }

    #[test]
    fn remove_takes_session_out_of_live_list() {
        let registry = SessionRegistry::new();
        let info = sample_info("spawned-researcher-0005");
        let _rx = registry
            .register(info.clone(), CancellationToken::new())
            .unwrap();

        assert_eq!(registry.list_live().len(), 1);
        let removed = registry.remove(&info.address, &info.run_id).unwrap();
        assert_eq!(removed.address, info.address);
        assert!(registry.list_live().is_empty());
    }

    #[test]
    fn remove_is_a_noop_for_a_stale_run_id() {
        // A newer run at the same address must never be deleted by an older
        // run's teardown racing behind it.
        let registry = SessionRegistry::new();
        let info = sample_info("spawned-researcher-0005b");
        let _rx = registry
            .register(info.clone(), CancellationToken::new())
            .unwrap();

        assert!(registry.remove(&info.address, "run-stale").is_none());
        assert_eq!(
            registry.list_live().len(),
            1,
            "the live entry must survive a removal attempt naming the wrong run id"
        );
    }

    #[test]
    fn list_live_orders_by_start_time() {
        let registry = SessionRegistry::new();
        let mut earlier = sample_info("spawned-a-0001");
        earlier.started_at = Utc::now() - chrono::Duration::seconds(60);
        let later = sample_info("spawned-b-0002");

        let _later_rx = registry
            .register(later.clone(), CancellationToken::new())
            .unwrap();
        let _earlier_rx = registry
            .register(earlier.clone(), CancellationToken::new())
            .unwrap();

        let mut live = registry.list_live().into_iter();
        assert_eq!(live.next().unwrap().address, earlier.address);
        assert_eq!(live.next().unwrap().address, later.address);
        assert!(live.next().is_none());
    }

    #[test]
    fn subagent_snapshot_reflects_live_sessions() {
        let registry = SessionRegistry::new();
        let info = sample_info("spawned-researcher-0006");
        let _rx = registry
            .register(info.clone(), CancellationToken::new())
            .unwrap();

        let snapshot = registry.subagent_snapshot();
        assert_eq!(snapshot.len(), 1);
        let first = snapshot.first().unwrap();
        assert_eq!(first.name, info.address.to_string());
        assert!(first.status.contains("running"));
    }

    #[test]
    fn generate_address_has_category_prefix_and_slug() {
        let addr = generate_address(&EventTrigger::Agent, "Research Helper");
        assert!(addr.as_ref().starts_with("spawned-research-helper-"));
    }

    #[test]
    fn generate_address_falls_back_when_qualifier_has_no_alnum() {
        let addr = generate_address(&EventTrigger::Pulse, "!!!");
        assert!(addr.as_ref().starts_with("scheduled-session-"));
    }

    #[test]
    fn generate_address_is_unique_across_calls() {
        let a = generate_address(&EventTrigger::Webhook("gh".into()), "gh");
        let b = generate_address(&EventTrigger::Webhook("gh".into()), "gh");
        assert_ne!(a, b);
    }

    #[test]
    fn generate_run_id_is_unique() {
        let a = generate_run_id();
        let b = generate_run_id();
        assert_ne!(a, b);
        assert!(a.starts_with("run-"));
    }

    fn sample_agent_message() -> Interrupt {
        Interrupt::AgentMessage(crate::bus::AgentMessageEvent {
            from: SessionAddress::from(MAIN_ADDRESS),
            from_category: "main".to_string(),
            content: "hello".to_string(),
            hop_count: 0,
        })
    }

    #[test]
    fn deliver_sends_to_a_live_session_channel() {
        let registry = SessionRegistry::new();
        let info = sample_info("spawned-researcher-0007");
        let mut rx = registry
            .register(info.clone(), CancellationToken::new())
            .unwrap();

        assert_eq!(
            registry.deliver(&info.address, sample_agent_message()),
            DeliverOutcome::Delivered
        );
        let received = rx.try_recv().expect("message should be queued");
        assert!(matches!(received, Interrupt::AgentMessage(_)));
    }

    #[test]
    fn deliver_returns_not_live_for_unknown_address() {
        let registry = SessionRegistry::new();
        assert_eq!(
            registry.deliver(&SessionAddress::from("ghost"), sample_agent_message()),
            DeliverOutcome::NotLive
        );
    }

    #[test]
    fn deliver_returns_completing_for_a_session_tearing_down() {
        let registry = SessionRegistry::new();
        let mut info = sample_info("spawned-researcher-0007b");
        info.state = SessionState::Completing;
        let _rx = registry
            .register(info.clone(), CancellationToken::new())
            .unwrap();

        assert_eq!(
            registry.deliver(&info.address, sample_agent_message()),
            DeliverOutcome::Completing,
            "a completing session must not accept a message into its own run"
        );
    }

    #[test]
    fn deliver_returns_full_when_the_channel_is_saturated() {
        let registry = SessionRegistry::new();
        let info = sample_info("spawned-researcher-0007c");
        let _rx = registry
            .register(info.clone(), CancellationToken::new())
            .unwrap();

        for _ in 0..INTERRUPT_CHANNEL_CAPACITY {
            assert_eq!(
                registry.deliver(&info.address, sample_agent_message()),
                DeliverOutcome::Delivered
            );
        }
        assert_eq!(
            registry.deliver(&info.address, sample_agent_message()),
            DeliverOutcome::Full,
            "a saturated channel must be distinguishable from an unknown address"
        );
    }

    #[tokio::test]
    async fn wait_until_clear_returns_once_the_entry_is_removed() {
        let registry = std::sync::Arc::new(SessionRegistry::new());
        let info = sample_info("spawned-researcher-0007d");
        let _rx = registry
            .register(info.clone(), CancellationToken::new())
            .unwrap();

        let waiter_registry = std::sync::Arc::clone(&registry);
        let address = info.address.clone();
        let waiter = tokio::spawn(async move {
            waiter_registry.wait_until_clear(&address).await;
        });

        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(
            !waiter.is_finished(),
            "should still be waiting while the entry is live"
        );

        registry.remove(&info.address, &info.run_id);
        tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("wait_until_clear should return once the entry is removed")
            .unwrap();
    }

    fn sample_resume_point(run_id: &str) -> ResumePoint {
        ResumePoint {
            previous_run_id: run_id.to_string(),
            previous_episode_id: Some("ep-1".to_string()),
            trigger: EventTrigger::Agent,
            source_label: "agent:researcher".to_string(),
            agent_skill: Some(SkillName::from("researcher")),
            model_tier: BackgroundModelTier::Large,
            spawner: Some(SessionAddress::from(MAIN_ADDRESS)),
            depth: 1,
            conversation_target: None,
            // Overwritten by `record_resume_point` on every real path;
            // tests that write a resume point straight to disk (bypassing
            // the registry) set this explicitly instead.
            recorded_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn resume_point_round_trips() {
        let registry = SessionRegistry::new();
        let address = SessionAddress::from("spawned-researcher-0008");
        let point = sample_resume_point("run-1");
        registry.record_resume_point(&address, point.clone()).await;

        let found = registry
            .resume_point(&address)
            .expect("resume point should be recorded");
        assert_eq!(found.previous_run_id, "run-1");
        assert_eq!(found.previous_episode_id.as_deref(), Some("ep-1"));
        assert_eq!(found.spawner, Some(SessionAddress::from(MAIN_ADDRESS)));
        assert_eq!(found.depth, 1);
    }

    #[test]
    fn resume_point_is_none_for_a_session_that_never_ran() {
        let registry = SessionRegistry::new();
        assert!(
            registry
                .resume_point(&SessionAddress::from("never-existed"))
                .is_none()
        );
    }

    #[tokio::test]
    async fn record_resume_point_on_a_registry_built_with_new_never_touches_disk() {
        // `new()` is what every other test in the codebase uses; it must
        // stay a pure in-memory registry with no persistence path.
        let registry = SessionRegistry::new();
        let address = SessionAddress::from("spawned-researcher-mem-only");
        registry
            .record_resume_point(&address, sample_resume_point("run-1"))
            .await;
        assert!(registry.resume_point(&address).is_some());
    }

    #[tokio::test]
    async fn record_resume_point_persists_write_through_to_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("resume_points.json");
        let registry = SessionRegistry::load(path.clone()).await;
        let address = SessionAddress::from("spawned-researcher-persisted");

        registry
            .record_resume_point(&address, sample_resume_point("run-1"))
            .await;

        let contents = tokio::fs::read_to_string(&path).await.unwrap();
        let on_disk: HashMap<SessionAddress, ResumePoint> =
            serde_json::from_str(&contents).unwrap();
        assert_eq!(
            on_disk
                .get(&address)
                .expect("the recorded point should be on disk")
                .previous_run_id,
            "run-1"
        );
    }

    #[tokio::test]
    async fn a_resume_point_survives_a_simulated_restart() {
        // A registry rebuilt from the same persisted file stands in for a
        // process restart; it must still resolve the address to its previous
        // run and episode.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("resume_points.json");
        let address = SessionAddress::from("external-discord-restart-test");

        {
            let registry = SessionRegistry::load(path.clone()).await;
            let mut point = sample_resume_point("run-before-restart");
            point.previous_episode_id = Some("ep-before-restart".to_string());
            registry.record_resume_point(&address, point).await;
        }
        // The first registry (and everything it held in memory) is dropped
        // here, standing in for the process exiting.

        let restarted = SessionRegistry::load(path).await;
        let found = restarted
            .resume_point(&address)
            .expect("the resume point must survive the simulated restart");
        assert_eq!(found.previous_run_id, "run-before-restart");
        assert_eq!(
            found.previous_episode_id.as_deref(),
            Some("ep-before-restart")
        );
    }

    #[tokio::test]
    async fn concurrent_recordings_all_reach_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("resume_points.json");
        let registry = std::sync::Arc::new(SessionRegistry::load(path.clone()).await);

        let recordings = (0..32).map(|i| {
            let registry = std::sync::Arc::clone(&registry);
            tokio::spawn(async move {
                let address = SessionAddress::from(format!("external-concurrent-{i}"));
                registry
                    .record_resume_point(&address, sample_resume_point(&format!("run-{i}")))
                    .await;
            })
        });
        for handle in recordings {
            handle.await.unwrap();
        }

        let restarted = SessionRegistry::load(path).await;
        for i in 0..32 {
            let address = SessionAddress::from(format!("external-concurrent-{i}"));
            assert!(
                restarted.resume_point(&address).is_some(),
                "resume point {i} must be on disk after concurrent recordings"
            );
        }
    }

    #[tokio::test]
    async fn load_from_a_missing_file_starts_with_no_resume_points() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does-not-exist.json");
        let registry = SessionRegistry::load(path).await;
        assert!(
            registry
                .resume_point(&SessionAddress::from("anything"))
                .is_none()
        );
    }

    #[tokio::test]
    async fn load_from_a_corrupt_file_starts_empty_instead_of_blocking_startup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("resume_points.json");
        tokio::fs::write(&path, "not valid json").await.unwrap();

        let registry = SessionRegistry::load(path).await;
        assert!(
            registry
                .resume_point(&SessionAddress::from("anything"))
                .is_none(),
            "a corrupt persisted file must not block startup or panic — it should just \
             start with no resume points, as if the file were empty"
        );
    }

    #[tokio::test]
    async fn load_prunes_entries_older_than_the_max_age() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("resume_points.json");

        let mut stale = sample_resume_point("run-stale");
        stale.recorded_at = Utc::now() - chrono::Duration::days(ResumePoint::MAX_AGE_DAYS + 1);
        let mut fresh = sample_resume_point("run-fresh");
        fresh.recorded_at = Utc::now() - chrono::Duration::days(1);

        let mut on_disk = HashMap::new();
        on_disk.insert(SessionAddress::from("spawned-stale"), stale);
        on_disk.insert(SessionAddress::from("spawned-fresh"), fresh);
        tokio::fs::write(&path, serde_json::to_string(&on_disk).unwrap())
            .await
            .unwrap();

        let registry = SessionRegistry::load(path).await;
        assert!(
            registry
                .resume_point(&SessionAddress::from("spawned-stale"))
                .is_none(),
            "an entry older than the max age must be pruned on load"
        );
        assert!(
            registry
                .resume_point(&SessionAddress::from("spawned-fresh"))
                .is_some(),
            "an entry within the max age must survive pruning"
        );
    }
}
