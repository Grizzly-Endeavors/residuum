//! Turns what the hub learns into entries of the team event log.
//!
//! The recorder reads the hub bus (lifecycle, notices) and the feed of agent
//! changes (sessions, inbox additions, main turns) and records one entry per
//! thing worth telling the user. Every summary is one plain sentence that
//! names the agent, in the product's voice.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use tokio::sync::broadcast::{self, error::RecvError};
use tokio::task::JoinHandle;

use super::event::{NewTeamEvent, TeamEventKind, TeamEventLevel, TeamEventPlace, TeamEventTarget};
use super::log::TeamEventLog;
use crate::background::registry::{SessionCategory, SessionInfo};
use crate::bus::{AgentResultStatus, EventTrigger};
use crate::hub::agent_watch::{AgentChange, AgentChangeKind, AgentChangeReceiver};
use crate::hub::types::{Actor, AgentErrorKind, AgentState, AgentSummary, HubEvent, NoticeLevel};
use crate::memory::types::Visibility;

/// A running recorder. It stops when dropped.
pub(crate) struct TeamEventRecorder {
    task: JoinHandle<()>,
}

impl TeamEventRecorder {
    /// Record into `log` what arrives on `hub_events` and `changes`.
    ///
    /// Both receivers must be subscribed before anything they should hear
    /// about happens: neither replays. Subscribe before the hub publishes its
    /// startup notices and before the agents start.
    pub(crate) fn spawn(
        log: Arc<TeamEventLog>,
        hub_events: broadcast::Receiver<HubEvent>,
        changes: AgentChangeReceiver,
    ) -> Self {
        let recorder = Recorder::new(log);
        Self {
            task: crate::util::spawn_monitored("team-events", recorder.run(hub_events, changes)),
        }
    }
}

impl Drop for TeamEventRecorder {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Record that the hub process started.
pub(crate) fn record_hub_started(log: &TeamEventLog) {
    log.record(NewTeamEvent {
        at: Utc::now(),
        agent: None,
        kind: TeamEventKind::HubStarted,
        level: TeamEventLevel::Info,
        summary: "Residuum started".to_string(),
        target: None,
    });
}

/// Identifies one run of one session of one agent: the agent, the session's
/// address and the run's id.
type RunKey = (String, String, String);

fn run_key(agent: &str, address: &str, run_id: &str) -> RunKey {
    (agent.to_string(), address.to_string(), run_id.to_string())
}

/// What the recorder keeps of a session run from its start, to word its end:
/// the end event carries neither the category nor what the run was for.
struct LiveRun {
    category: SessionCategory,
    trigger: EventTrigger,
    source_label: String,
    purpose: String,
}

impl LiveRun {
    fn of(info: &SessionInfo) -> Self {
        Self {
            category: info.category,
            trigger: info.trigger.clone(),
            source_label: info.source_label.clone(),
            purpose: plain(&info.purpose),
        }
    }
}

struct Recorder {
    log: Arc<TeamEventLog>,
    /// The state each agent was last reported in, to tell a change of state
    /// from a change of settings. An agent never reported is stopped.
    states: BTreeMap<String, AgentState>,
    /// The session runs that have started and not ended.
    runs: HashMap<RunKey, LiveRun>,
}

impl Recorder {
    fn new(log: Arc<TeamEventLog>) -> Self {
        Self {
            log,
            states: BTreeMap::new(),
            runs: HashMap::new(),
        }
    }

    async fn run(
        mut self,
        mut hub_events: broadcast::Receiver<HubEvent>,
        mut changes: AgentChangeReceiver,
    ) {
        loop {
            tokio::select! {
                event = hub_events.recv() => match event {
                    Ok(event) => self.on_hub_event(event),
                    Err(RecvError::Lagged(missed)) => {
                        tracing::warn!(missed, "the team event log fell behind the hub's events and missed some");
                    }
                    Err(RecvError::Closed) => break,
                },
                change = changes.recv() => match change {
                    Some(change) => self.on_agent_change(change),
                    None => break,
                },
            }
        }
        tracing::debug!("the team event log stopped recording");
    }

    fn record(
        &self,
        at: DateTime<Utc>,
        agent: Option<&str>,
        kind: TeamEventKind,
        level: TeamEventLevel,
        summary: String,
        target: Option<TeamEventTarget>,
    ) {
        self.log.record(NewTeamEvent {
            at,
            agent: agent.map(str::to_string),
            kind,
            level,
            summary,
            target,
        });
    }

    /// An entry of `kind` about `agent` that points at the agent's chat.
    fn record_in_chat(
        &self,
        agent: &str,
        kind: TeamEventKind,
        level: TeamEventLevel,
        summary: String,
    ) {
        self.record(
            Utc::now(),
            Some(agent),
            kind,
            level,
            summary,
            Some(chat_place(agent)),
        );
    }

    // ─── The hub bus ──────────────────────────────────────────────────

    fn on_hub_event(&mut self, event: HubEvent) {
        match event {
            HubEvent::AgentState { agent } => self.on_agent_state(&agent),
            HubEvent::AgentCreated { agent, by } => self.record_in_chat(
                &agent.name,
                TeamEventKind::AgentCreated,
                TeamEventLevel::Info,
                with_actor(&agent.name, "was created", &by),
            ),
            HubEvent::AgentRestored { agent, by } => self.record_in_chat(
                &agent.name,
                TeamEventKind::AgentRestored,
                TeamEventLevel::Info,
                with_actor(&agent.name, "was restored", &by),
            ),
            HubEvent::AgentDeleted { name, by } => self.on_agent_deleted(&name, &by),
            HubEvent::Notice {
                level,
                message,
                agent,
            } => self.record(
                Utc::now(),
                agent.as_deref(),
                TeamEventKind::HubNotice,
                level_of(level),
                plain(&message),
                None,
            ),
            HubEvent::AgentStopping { .. }
            | HubEvent::AgentActivity { .. }
            | HubEvent::HubConfigReloaded { .. } => {}
        }
    }

    /// An agent's state, autostart or visibility was published. Only a move
    /// into `running`, `stopped` or `failed` is worth an entry.
    fn on_agent_state(&mut self, agent: &AgentSummary) {
        let before = self
            .states
            .insert(agent.name.clone(), agent.state)
            .unwrap_or(AgentState::Stopped);
        if before == agent.state {
            return;
        }
        let name = &agent.name;
        match agent.state {
            AgentState::Running => self.record_in_chat(
                name,
                TeamEventKind::AgentStarted,
                TeamEventLevel::Info,
                format!("{name} started"),
            ),
            AgentState::Stopped => {
                if matches!(before, AgentState::Running | AgentState::Starting) {
                    self.record_in_chat(
                        name,
                        TeamEventKind::AgentStopped,
                        TeamEventLevel::Info,
                        format!("{name} stopped"),
                    );
                }
            }
            AgentState::Failed => self.record_in_chat(
                name,
                TeamEventKind::AgentFailed,
                TeamEventLevel::Error,
                failure_summary(agent, before),
            ),
            AgentState::Starting => {}
        }
    }

    fn on_agent_deleted(&mut self, name: &str, by: &Actor) {
        self.states.remove(name);
        self.runs.retain(|(agent, _, _), _| agent != name);
        self.record(
            Utc::now(),
            Some(name),
            TeamEventKind::AgentDeleted,
            TeamEventLevel::Info,
            with_actor(name, "was deleted", by),
            None,
        );
    }

    // ─── The feed of agent changes ────────────────────────────────────

    fn on_agent_change(&mut self, change: AgentChange) {
        let AgentChange { agent, kind } = change;
        match kind {
            AgentChangeKind::TurnEnded(turn) => {
                // A reply is told once per turn, and only when the user was
                // part of the turn: a background turn's reply is not for them.
                if turn.reply.is_some() && turn.visibility == Visibility::User {
                    self.record(
                        turn.at,
                        Some(&agent),
                        TeamEventKind::AgentReplied,
                        TeamEventLevel::Info,
                        format!("{agent} replied in your conversation"),
                        Some(chat_place(&agent)),
                    );
                }
            }
            AgentChangeKind::UserInboxAdded { item_id } => self.record(
                Utc::now(),
                Some(&agent),
                TeamEventKind::InboxItemAdded,
                TeamEventLevel::Info,
                format!("{agent} added an item to your inbox"),
                Some(TeamEventTarget::InboxItem {
                    agent: agent.clone(),
                    item_id,
                }),
            ),
            AgentChangeKind::SessionStarted(info) => self.on_session_started(&agent, &info),
            AgentChangeKind::SessionCompleted {
                address,
                run_id,
                status,
                episode_id: _,
            } => self.on_session_completed(&agent, address.as_ref(), &run_id, &status),
            AgentChangeKind::Resync
            | AgentChangeKind::SessionStateChanged { .. }
            | AgentChangeKind::OutboundTaskChanged(_)
            | AgentChangeKind::WatchedPathChanged(_) => {}
        }
    }

    fn on_session_started(&mut self, agent: &str, info: &SessionInfo) {
        let run = LiveRun::of(info);
        let scheduled = run.category == SessionCategory::Scheduled;
        let summary = with_detail(format!("{agent} started a session"), &run.purpose);
        self.runs
            .insert(run_key(agent, info.address.as_ref(), &info.run_id), run);
        // A scheduled run is told once, when it finishes.
        if !scheduled {
            self.record(
                info.started_at,
                Some(agent),
                TeamEventKind::SessionStarted,
                TeamEventLevel::Info,
                summary,
                Some(session_target(agent, &info.run_id)),
            );
        }
    }

    fn on_session_completed(
        &mut self,
        agent: &str,
        address: &str,
        run_id: &str,
        status: &AgentResultStatus,
    ) {
        let Some(run) = self.runs.remove(&run_key(agent, address, run_id)) else {
            // Every run the hub hears of starts on the feed first, so this
            // can only be a run that started before the agent was watched.
            tracing::debug!(%agent, %address, %run_id, "a session ended that the team event log never saw start; not recorded");
            return;
        };
        let (kind, level, summary) = if run.category == SessionCategory::Scheduled {
            let (level, summary) = scheduled_run_finished(agent, &run, status);
            (TeamEventKind::ScheduledRunFinished, level, summary)
        } else {
            let (level, summary) = session_finished(agent, &run, status);
            (TeamEventKind::SessionFinished, level, summary)
        };
        self.record(
            Utc::now(),
            Some(agent),
            kind,
            level,
            summary,
            Some(session_target(agent, run_id)),
        );
    }
}

// ─── Wording ──────────────────────────────────────────────────────────

fn chat_place(agent: &str) -> TeamEventTarget {
    TeamEventTarget::AgentPlace {
        agent: agent.to_string(),
        place: TeamEventPlace::Chat,
    }
}

fn session_target(agent: &str, run_id: &str) -> TeamEventTarget {
    TeamEventTarget::Session {
        agent: agent.to_string(),
        run_id: run_id.to_string(),
    }
}

fn level_of(level: NoticeLevel) -> TeamEventLevel {
    match level {
        NoticeLevel::Info => TeamEventLevel::Info,
        NoticeLevel::Warn => TeamEventLevel::Warn,
        NoticeLevel::Error => TeamEventLevel::Error,
    }
}

/// `text` on one line, with its runs of whitespace collapsed.
fn plain(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `base`, then a colon and `detail` when there is one.
fn with_detail(base: String, detail: &str) -> String {
    if detail.is_empty() {
        base
    } else {
        format!("{base}: {detail}")
    }
}

/// "atlas was created", or "atlas was created by scout" when an agent did it.
/// What the user does themselves needs no mention.
fn with_actor(name: &str, done: &str, by: &Actor) -> String {
    match by {
        Actor::User => format!("{name} {done}"),
        Actor::Agent(actor) => format!("{name} {done} by {actor}"),
    }
}

/// What went wrong with an agent, for its `agent_failed` entry. `before` is
/// the state it failed out of.
fn failure_summary(agent: &AgentSummary, before: AgentState) -> String {
    let name = &agent.name;
    let Some(error) = &agent.last_error else {
        return format!("{name} failed");
    };
    match error.kind {
        AgentErrorKind::Config | AgentErrorKind::PortConflict | AgentErrorKind::Other => {
            let reason = plain(&error.reason);
            format!("{name} couldn't start: {}", reason.trim_end_matches('.'))
        }
        AgentErrorKind::Crash if before == AgentState::Running => {
            format!("{name} stopped unexpectedly")
        }
        AgentErrorKind::Crash => format!("{name} couldn't start because of an internal error"),
    }
}

/// The level and summary of a finished session that was not scheduled.
fn session_finished(
    agent: &str,
    run: &LiveRun,
    status: &AgentResultStatus,
) -> (TeamEventLevel, String) {
    match status {
        AgentResultStatus::Completed => (
            TeamEventLevel::Info,
            with_detail(format!("{agent} finished a session"), &run.purpose),
        ),
        AgentResultStatus::Cancelled => (
            TeamEventLevel::Warn,
            with_detail(format!("{agent}'s session was stopped"), &run.purpose),
        ),
        AgentResultStatus::Failed { error, .. } => (
            TeamEventLevel::Error,
            with_detail(format!("{agent}'s session failed"), &plain(error)),
        ),
    }
}

/// The level and summary of a finished pulse or scheduled action.
fn scheduled_run_finished(
    agent: &str,
    run: &LiveRun,
    status: &AgentResultStatus,
) -> (TeamEventLevel, String) {
    let what = scheduled_run_name(run);
    match status {
        AgentResultStatus::Completed => {
            (TeamEventLevel::Info, format!("{agent} finished the {what}"))
        }
        AgentResultStatus::Cancelled => (
            TeamEventLevel::Info,
            format!("{agent}'s {what} was stopped"),
        ),
        AgentResultStatus::Failed { error, .. } => (
            TeamEventLevel::Error,
            with_detail(format!("{agent}'s {what} failed"), &plain(error)),
        ),
    }
}

/// `pulse "email_check"`, `scheduled action "nightly digest"`: what a
/// scheduled run was, by the name after the colon of its source label.
fn scheduled_run_name(run: &LiveRun) -> String {
    let kind = match run.trigger {
        EventTrigger::Pulse => "pulse",
        EventTrigger::Action => "scheduled action",
        EventTrigger::Agent
        | EventTrigger::Webhook(_)
        | EventTrigger::Conversation
        | EventTrigger::Artifact(_) => "scheduled run",
    };
    let name = run
        .source_label
        .split_once(':')
        .map_or(run.source_label.as_str(), |(_, name)| name);
    format!("{kind} \"{name}\"")
}

#[cfg(test)]
mod tests;
