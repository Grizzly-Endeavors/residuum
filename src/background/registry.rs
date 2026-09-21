//! Session registry: the single source of truth for every live agent session.
//!
//! Tracks address, run id, category, source label, lifecycle state, spawner,
//! depth, and purpose for every session that is currently running or idle.
//! Discovery tools (`list_agents`, `stop_agent`) and the bug-report client
//! context read this registry rather than any lower-level task map.

use std::collections::HashMap;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use tokio_util::sync::CancellationToken;

use crate::bus::{EventTrigger, SessionAddress, SkillName};

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
    /// When this run started.
    pub started_at: DateTime<Utc>,
}

/// A registered session's bookkeeping: its metadata plus the token that
/// cancels its in-flight turn or idle wait.
struct SessionEntry {
    info: SessionInfo,
    stop_token: CancellationToken,
}

/// Registry of every live (running, idle, or completing) session.
///
/// Completed sessions are removed; their addresses remain valid for
/// messaging (once messaging exists, in Phase 3) but they no longer appear
/// in discovery.
#[derive(Default)]
pub struct SessionRegistry {
    sessions: Mutex<HashMap<SessionAddress, SessionEntry>>,
}

impl SessionRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new session run, replacing any existing entry at the same
    /// address (a resumed run in a later phase would land here).
    pub fn register(&self, info: SessionInfo, stop_token: CancellationToken) {
        let address = info.address.clone();
        let mut guard = self.lock();
        guard.insert(address, SessionEntry { info, stop_token });
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

    /// Remove a session from the registry once it has fully completed.
    pub fn remove(&self, address: &SessionAddress) -> Option<SessionInfo> {
        self.lock().remove(address).map(|entry| entry.info)
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
        registry.register(info.clone(), CancellationToken::new());

        let got = registry.get(&info.address).unwrap();
        assert_eq!(got.address, info.address);
        assert_eq!(got.state, SessionState::Running);
    }

    #[test]
    fn set_state_updates_existing_session() {
        let registry = SessionRegistry::new();
        let info = sample_info("spawned-researcher-0002");
        registry.register(info.clone(), CancellationToken::new());

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
        registry.register(info.clone(), token.clone());

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
        registry.register(info.clone(), CancellationToken::new());

        assert!(!registry.stop(&info.address));
    }

    #[test]
    fn remove_takes_session_out_of_live_list() {
        let registry = SessionRegistry::new();
        let info = sample_info("spawned-researcher-0005");
        registry.register(info.clone(), CancellationToken::new());

        assert_eq!(registry.list_live().len(), 1);
        let removed = registry.remove(&info.address).unwrap();
        assert_eq!(removed.address, info.address);
        assert!(registry.list_live().is_empty());
    }

    #[test]
    fn list_live_orders_by_start_time() {
        let registry = SessionRegistry::new();
        let mut earlier = sample_info("spawned-a-0001");
        earlier.started_at = Utc::now() - chrono::Duration::seconds(60);
        let later = sample_info("spawned-b-0002");

        registry.register(later.clone(), CancellationToken::new());
        registry.register(earlier.clone(), CancellationToken::new());

        let mut live = registry.list_live().into_iter();
        assert_eq!(live.next().unwrap().address, earlier.address);
        assert_eq!(live.next().unwrap().address, later.address);
        assert!(live.next().is_none());
    }

    #[test]
    fn subagent_snapshot_reflects_live_sessions() {
        let registry = SessionRegistry::new();
        let info = sample_info("spawned-researcher-0006");
        registry.register(info.clone(), CancellationToken::new());

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
}
