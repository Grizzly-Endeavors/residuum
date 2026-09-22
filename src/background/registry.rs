//! Session registry: the single source of truth for every live agent session.
//!
//! Tracks address, run id, category, source label, lifecycle state, spawner,
//! depth, and purpose for every session that is currently running or idle.
//! Discovery tools (`list_agents`, `stop_agent`) and the bug-report client
//! context read this registry rather than any lower-level task map.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::agent::interrupt::Interrupt;
use crate::bus::{EventTrigger, SessionAddress, SkillName};
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

/// How a session was started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionCategory {
    /// Pulses and scheduled actions.
    Scheduled,
    /// Non-owner-DM conversations and webhooks.
    External,
    /// Started by an agent: `subagent_spawn` or the subconscious learner.
    Spawned,
}

impl SessionCategory {
    /// Derive the category from the trigger that started the session.
    #[must_use]
    pub fn from_trigger(trigger: &EventTrigger) -> Self {
        match trigger {
            EventTrigger::Pulse | EventTrigger::Action => Self::Scheduled,
            EventTrigger::Agent => Self::Spawned,
            EventTrigger::Webhook(_) => Self::External,
        }
    }

    /// Lowercase label used in addresses, logs, and the store.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Scheduled => "scheduled",
            Self::External => "external",
            Self::Spawned => "spawned",
        }
    }
}

impl std::fmt::Display for SessionCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A session's lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    /// When this run started.
    pub started_at: DateTime<Utc>,
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
#[derive(Debug, Clone)]
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
}

/// Registry of every live (running, idle, or completing) session, plus a
/// standing record of what's needed to resume a completed one.
///
/// Live sessions are removed once completed; their resume points are not —
/// an address must keep working for messaging across idle gaps and after
/// completion, for as long as the process runs.
#[derive(Default)]
pub struct SessionRegistry {
    sessions: Mutex<HashMap<SessionAddress, SessionEntry>>,
    resume_points: Mutex<HashMap<SessionAddress, ResumePoint>>,
}

impl SessionRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new session run, replacing any existing entry at the same
    /// address (a resumed run lands here the same way a fresh one does).
    ///
    /// Returns the receiver half of the session's interrupt channel, for the
    /// runtime to drive the run's turns with.
    #[must_use]
    pub fn register(
        &self,
        info: SessionInfo,
        stop_token: CancellationToken,
    ) -> mpsc::Receiver<Interrupt> {
        let (interrupt_tx, interrupt_rx) = mpsc::channel(INTERRUPT_CHANNEL_CAPACITY);
        let address = info.address.clone();
        let mut guard = self.lock();
        guard.insert(
            address,
            SessionEntry {
                info,
                stop_token,
                interrupt_tx,
            },
        );
        interrupt_rx
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
    /// bug elsewhere. Logs once if the wait runs unusually long, since that
    /// would mean the previous run's teardown is stuck.
    pub async fn wait_until_clear(&self, address: &SessionAddress) {
        const POLL_INTERVAL: Duration = Duration::from_millis(50);
        const WARN_AFTER: Duration = Duration::from_secs(30);

        let start = std::time::Instant::now();
        let mut warned = false;
        while self.get(address).is_some() {
            if !warned && start.elapsed() > WARN_AFTER {
                tracing::warn!(
                    address = %address,
                    waited_secs = start.elapsed().as_secs(),
                    "still waiting for a completing session to leave the registry"
                );
                warned = true;
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// Record what's needed to resume a session as a new run once its
    /// current run completes. Overwrites any previous resume point at the
    /// same address, since only the most recent run's pointer matters.
    pub fn record_resume_point(&self, address: &SessionAddress, point: ResumePoint) {
        self.resume_points
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(address.clone(), point);
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
        let mut guard = self.lock();
        if guard.get(address).is_none_or(|e| e.info.run_id != run_id) {
            return None;
        }
        guard.remove(address).map(|entry| entry.info)
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
            started_at: Utc::now(),
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
    }

    #[test]
    fn register_and_get_round_trips() {
        let registry = SessionRegistry::new();
        let info = sample_info("spawned-researcher-0001");
        let _rx = registry.register(info.clone(), CancellationToken::new());

        let got = registry.get(&info.address).unwrap();
        assert_eq!(got.address, info.address);
        assert_eq!(got.state, SessionState::Running);
    }

    #[test]
    fn set_state_updates_existing_session() {
        let registry = SessionRegistry::new();
        let info = sample_info("spawned-researcher-0002");
        let _rx = registry.register(info.clone(), CancellationToken::new());

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

    #[test]
    fn stop_cancels_token_for_running_session() {
        let registry = SessionRegistry::new();
        let info = sample_info("spawned-researcher-0003");
        let token = CancellationToken::new();
        let _rx = registry.register(info.clone(), token.clone());

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
        let _rx = registry.register(info.clone(), CancellationToken::new());

        assert!(!registry.stop(&info.address));
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
        let _running_rx = registry.register(running.clone(), running_token.clone());
        let _idle_rx = registry.register(idle.clone(), idle_token.clone());
        let _completing_rx = registry.register(completing.clone(), completing_token.clone());

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
        let _rx = registry.register(info.clone(), CancellationToken::new());

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
        let _rx = registry.register(info.clone(), CancellationToken::new());

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

        let _later_rx = registry.register(later.clone(), CancellationToken::new());
        let _earlier_rx = registry.register(earlier.clone(), CancellationToken::new());

        let mut live = registry.list_live().into_iter();
        assert_eq!(live.next().unwrap().address, earlier.address);
        assert_eq!(live.next().unwrap().address, later.address);
        assert!(live.next().is_none());
    }

    #[test]
    fn subagent_snapshot_reflects_live_sessions() {
        let registry = SessionRegistry::new();
        let info = sample_info("spawned-researcher-0006");
        let _rx = registry.register(info.clone(), CancellationToken::new());

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
        let mut rx = registry.register(info.clone(), CancellationToken::new());

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
        let _rx = registry.register(info.clone(), CancellationToken::new());

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
        let _rx = registry.register(info.clone(), CancellationToken::new());

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
        let _rx = registry.register(info.clone(), CancellationToken::new());

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

    #[test]
    fn resume_point_round_trips() {
        let registry = SessionRegistry::new();
        let address = SessionAddress::from("spawned-researcher-0008");
        let point = ResumePoint {
            previous_run_id: "run-1".to_string(),
            previous_episode_id: Some("ep-1".to_string()),
            trigger: EventTrigger::Agent,
            source_label: "agent:researcher".to_string(),
            agent_skill: Some(SkillName::from("researcher")),
            model_tier: BackgroundModelTier::Large,
            spawner: Some(SessionAddress::from(MAIN_ADDRESS)),
            depth: 1,
        };
        registry.record_resume_point(&address, point.clone());

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
}
