use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use tokio::sync::broadcast::{self, error::RecvError};
use tokio::task::JoinHandle;

use super::service::PushService;
use super::types::{PushEvent, PushMessage, PushPayload};
use crate::agent_watch::{AgentChange, AgentChangeKind, AgentChangeReceiver};
use crate::directory::AgentDirectory;
use crate::mail;
use crate::overview::plain_preview;
use crate::types::HubEvent;
use residuum_background::team::{AgentErrorKind, AgentState, AgentSummary};
use residuum_bus::remote_task::TrackedTask;
use residuum_memory::types::Visibility;
use residuum_workspace::inbox::{InboxKind, InboxRef};

const UNREADABLE_ITEM_TITLE: &str = "New inbox item";

pub struct TriggerInputs {
    pub push: Arc<PushService>,
    pub directory: Arc<dyn AgentDirectory>,
    pub hub_events: broadcast::Receiver<HubEvent>,
    pub changes: AgentChangeReceiver,
    pub notice: Box<dyn Fn(String) + Send + Sync>,
    pub decisions: Option<broadcast::Sender<TriggerDecision>>,
}

#[derive(Debug, Clone)]
pub struct TriggerDecision {
    pub input: TriggerInput,
    pub triggers: Vec<Trigger>,
}

#[derive(Debug, Clone)]
pub enum TriggerInput {
    HubEvent(HubEvent),
    Change(AgentChange),
}

impl TriggerInput {
    fn agent(&self) -> Option<&str> {
        match self {
            Self::HubEvent(
                HubEvent::AgentState { agent }
                | HubEvent::AgentCreated { agent, .. }
                | HubEvent::AgentRestored { agent, .. },
            ) => Some(&agent.name),
            Self::HubEvent(HubEvent::AgentDeleted { name, .. }) => Some(name),
            Self::HubEvent(_) => None,
            Self::Change(change) => Some(&change.agent),
        }
    }
}

pub struct PushTriggers {
    task: JoinHandle<()>,
}

impl PushTriggers {
    #[must_use]
    pub fn spawn(inputs: TriggerInputs) -> Self {
        Self {
            task: residuum_core::util::spawn_monitored("push-triggers", run(inputs)),
        }
    }
}

impl Drop for PushTriggers {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn run(inputs: TriggerInputs) {
    let TriggerInputs {
        push,
        directory,
        mut hub_events,
        mut changes,
        notice,
        decisions,
    } = inputs;
    let mut notices = push.subscribe_notices();
    let sender = Sender { push, directory };
    let mut rules = Rules::default();
    loop {
        let (input, triggers): (Option<TriggerInput>, Vec<Trigger>) = tokio::select! {
            event = hub_events.recv() => match event {
                Ok(event) => {
                    let triggers = rules.on_hub_event(&event).into_iter().collect();
                    (Some(TriggerInput::HubEvent(event)), triggers)
                }
                Err(RecvError::Lagged(missed)) => {
                    tracing::warn!(missed, "push notifications fell behind the hub's events; reading every agent's state again");
                    (None, rules.reconcile(&sender.directory.list()))
                }
                Err(RecvError::Closed) => break,
            },
            change = changes.recv() => match change {
                Some(change) => {
                    let triggers = rules.on_change(change.clone()).into_iter().collect();
                    (Some(TriggerInput::Change(change)), triggers)
                }
                None => break,
            },
            message = notices.recv() => {
                match message {
                    Ok(message) => notice(message),
                    Err(RecvError::Lagged(missed)) => {
                        tracing::warn!(missed, "notices about push delivery were lost because many devices failed at once; see the hub log and each device's last failure");
                    }
                    Err(RecvError::Closed) => break,
                }
                (None, Vec::new())
            }
        };
        for trigger in &triggers {
            sender.send(trigger.clone()).await;
        }
        if let Some(input) = input {
            let decision = TriggerDecision { input, triggers };
            let agent = decision.input.agent().unwrap_or_default();
            if decision.triggers.is_empty() {
                tracing::trace!(agent, input = ?decision.input, "nothing in this is worth a push");
            } else {
                tracing::debug!(agent, triggers = ?decision.triggers, "pushes started");
            }
            if let Some(decisions) = &decisions {
                decisions.send(decision).ok();
            }
        }
    }
    tracing::debug!("push triggers stopped");
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trigger {
    InboxItem {
        agent: String,
        item_id: String,
    },
    AgentFailed {
        agent: String,
        kind: AgentErrorKind,
        was_running: bool,
    },
    OutboundUnreachable {
        agent: String,
        remote: String,
        task_id: String,
        since: DateTime<Utc>,
    },
    ReplyWhileAway {
        agent: String,
        reply: String,
    },
}

impl Trigger {
    fn event(&self) -> PushEvent {
        match self {
            Self::InboxItem { .. } => PushEvent::InboxItem,
            Self::AgentFailed { .. } => PushEvent::AgentFailed,
            Self::OutboundUnreachable { .. } => PushEvent::OutboundUnreachable,
            Self::ReplyWhileAway { .. } => PushEvent::ReplyWhileAway,
        }
    }

    fn agent(&self) -> &str {
        match self {
            Self::InboxItem { agent, .. }
            | Self::AgentFailed { agent, .. }
            | Self::OutboundUnreachable { agent, .. }
            | Self::ReplyWhileAway { agent, .. } => agent,
        }
    }
}

#[derive(Default)]
struct Rules {
    states: BTreeMap<String, AgentState>,
    pushed_outbound: HashSet<(String, String)>,
}

impl Rules {
    fn on_hub_event(&mut self, event: &HubEvent) -> Option<Trigger> {
        match event {
            HubEvent::AgentState { agent }
            | HubEvent::AgentCreated { agent, .. }
            | HubEvent::AgentRestored { agent, .. } => self.observe(agent),
            HubEvent::AgentDeleted { name, .. } => {
                self.states.remove(name);
                self.pushed_outbound.retain(|(agent, _)| agent != name);
                None
            }
            HubEvent::AgentStopping { .. }
            | HubEvent::AgentActivity { .. }
            | HubEvent::Notice { .. }
            | HubEvent::HubConfigReloaded { .. } => None,
        }
    }

    fn reconcile(&mut self, agents: &[AgentSummary]) -> Vec<Trigger> {
        self.states
            .retain(|name, _| agents.iter().any(|agent| &agent.name == name));
        agents
            .iter()
            .filter_map(|agent| self.observe(agent))
            .collect()
    }

    fn observe(&mut self, agent: &AgentSummary) -> Option<Trigger> {
        let before = self
            .states
            .insert(agent.name.clone(), agent.state)
            .unwrap_or(AgentState::Stopped);
        (agent.state == AgentState::Failed && before != AgentState::Failed).then(|| {
            Trigger::AgentFailed {
                agent: agent.name.clone(),
                kind: agent
                    .last_error
                    .as_ref()
                    .map_or(AgentErrorKind::Other, |error| error.kind),
                was_running: before == AgentState::Running,
            }
        })
    }

    fn on_change(&mut self, change: AgentChange) -> Option<Trigger> {
        let AgentChange { agent, kind } = change;
        match kind {
            AgentChangeKind::UserInboxAdded { item_id } => {
                Some(Trigger::InboxItem { agent, item_id })
            }
            AgentChangeKind::TurnEnded(turn) => {
                if turn.visibility != Visibility::User || turn.client_connected {
                    return None;
                }
                turn.reply
                    .map(|reply| Trigger::ReplyWhileAway { agent, reply })
            }
            AgentChangeKind::OutboundTaskChanged(task) => self.on_outbound_task(agent, &task),
            AgentChangeKind::Resync
            | AgentChangeKind::SessionStarted(_)
            | AgentChangeKind::SessionStateChanged { .. }
            | AgentChangeKind::SessionCompleted { .. }
            | AgentChangeKind::PulseFireFinished { .. }
            | AgentChangeKind::WatchedPathChanged(_) => None,
        }
    }

    fn on_outbound_task(&mut self, agent: String, task: &TrackedTask) -> Option<Trigger> {
        let key = (agent.clone(), task.task_id.clone());
        if !(task.unreachable_notified && task.is_open()) {
            self.pushed_outbound.remove(&key);
            return None;
        }
        if !self.pushed_outbound.insert(key) {
            return None;
        }
        Some(Trigger::OutboundUnreachable {
            agent,
            remote: task.agent.clone(),
            task_id: task.task_id.clone(),
            since: task.first_unreachable_at.unwrap_or(task.updated_at),
        })
    }
}

struct Sender {
    push: Arc<PushService>,
    directory: Arc<dyn AgentDirectory>,
}

impl Sender {
    async fn send(&self, trigger: Trigger) {
        let event = trigger.event();
        let skip = self.push.presence().active_devices();
        if !self.push.would_notify(event, &skip).await {
            tracing::debug!(agent = %trigger.agent(), event = ?event, "no push sent: no device wants it, or the ones that do are in front of the app");
            return;
        }
        let Some(payload) = self.payload(trigger).await else {
            return;
        };
        self.push.notify(PushMessage::new(payload), skip);
    }

    async fn payload(&self, trigger: Trigger) -> Option<PushPayload> {
        let badge = self.badge().await;
        Some(match trigger {
            Trigger::InboxItem { agent, item_id } => {
                let text = match mail::active_text(self.directory.as_ref(), &agent, &item_id).await
                {
                    Ok(Some(text)) => Some(text),
                    Ok(None) => {
                        tracing::debug!(agent = %agent, item_id = %item_id, "no push sent: the item left the inbox before it was worded");
                        return None;
                    }
                    Err(e) => {
                        tracing::warn!(agent = %agent, item_id = %item_id, error = %e, "couldn't read a new inbox item, so its push says only that it arrived");
                        None
                    }
                };
                inbox_item_payload(&agent, &item_id, text, badge)
            }
            Trigger::AgentFailed {
                agent,
                kind,
                was_running,
            } => agent_failed_payload(&agent, kind, was_running, badge),
            Trigger::OutboundUnreachable {
                agent,
                remote,
                task_id,
                since,
            } => {
                let since = self.local_time(&agent, since);
                outbound_unreachable_payload(&agent, &remote, &task_id, &since, badge)
            }
            Trigger::ReplyWhileAway { agent, reply } => {
                reply_while_away_payload(&agent, &reply, badge)
            }
        })
    }

    async fn badge(&self) -> u32 {
        mail::unread_total(self.directory.as_ref()).await
    }

    fn local_time(&self, agent: &str, at: DateTime<Utc>) -> String {
        let tz = match self.directory.agent_files(agent) {
            Ok(files) => files.timezone,
            Err(e) => {
                tracing::debug!(agent = %agent, error = %e, "couldn't read the hub's timezone, so a push words its time in UTC");
                chrono_tz::UTC
            }
        };
        clock_text(at.with_timezone(&tz), Utc::now().with_timezone(&tz))
    }
}

fn clock_text(at: DateTime<chrono_tz::Tz>, now: DateTime<chrono_tz::Tz>) -> String {
    if at.date_naive() == now.date_naive() {
        at.format("%H:%M").to_string()
    } else {
        at.format("%b %-d, %H:%M").to_string()
    }
}

fn inbox_item_target(agent: &str, item_id: &str) -> String {
    match InboxRef::new(agent, InboxKind::User, item_id) {
        Ok(item) => mail::thread_link(&item),
        Err(e) => {
            tracing::warn!(agent = %agent, item_id = %item_id, error = %e, "a new inbox item has no ref, so its push opens the Inbox");
            "/inbox".to_string()
        }
    }
}

fn inbox_item_payload(
    agent: &str,
    item_id: &str,
    text: Option<(String, String)>,
    badge: u32,
) -> PushPayload {
    let (title, preview) = match text {
        Some((title, body)) => (plain_preview(&title), plain_preview(&body)),
        None => (String::new(), String::new()),
    };
    let title = if title.is_empty() {
        UNREADABLE_ITEM_TITLE.to_string()
    } else {
        title
    };
    let body = if preview.is_empty() {
        format!("From {agent}.")
    } else {
        format!("From {agent}: {preview}")
    };
    PushPayload::new(
        PushEvent::InboxItem,
        agent,
        title,
        &body,
        inbox_item_target(agent, item_id),
        format!("inbox:{agent}:{item_id}"),
        badge,
    )
}

fn agent_failed_payload(
    agent: &str,
    kind: AgentErrorKind,
    was_running: bool,
    badge: u32,
) -> PushPayload {
    let title = if was_running {
        format!("{agent} stopped unexpectedly")
    } else {
        format!("{agent} couldn't start")
    };
    let body = match kind {
        AgentErrorKind::Config => "Its settings need fixing before it can run.",
        AgentErrorKind::PortConflict => "Another agent is using its Teams port.",
        AgentErrorKind::Crash => "It hit an internal error. Open Residuum to restart it.",
        AgentErrorKind::Other => "Open Residuum to see what went wrong.",
    };
    PushPayload::new(
        PushEvent::AgentFailed,
        agent,
        title,
        body,
        format!("/agent/{agent}"),
        format!("failed:{agent}"),
        badge,
    )
}

fn outbound_unreachable_payload(
    agent: &str,
    remote: &str,
    task_id: &str,
    since: &str,
    badge: u32,
) -> PushPayload {
    PushPayload::new(
        PushEvent::OutboundUnreachable,
        agent,
        format!("{agent} can't reach {remote}"),
        &format!("A task has been waiting since {since}."),
        format!("/agent/{agent}/activity"),
        format!("outbound:{agent}:{task_id}"),
        badge,
    )
}

fn reply_while_away_payload(agent: &str, reply: &str, badge: u32) -> PushPayload {
    PushPayload::new(
        PushEvent::ReplyWhileAway,
        agent,
        format!("{agent} replied"),
        &plain_preview(reply),
        format!("/agent/{agent}"),
        format!("reply:{agent}"),
        badge,
    )
}

#[cfg(test)]
mod tests;
