//! The four things that send a Web Push, and the rule for each.
//!
//! [`PushTriggers`] reads the hub bus and the feed of agent changes. The
//! rules ([`Rules`]) say which of what arrives is worth a notification, and
//! the sender ([`Sender`]) words it, counts the unread inbox for the app
//! badge, and hands it to [`PushService::notify`], skipping the devices whose
//! user is looking at the app.
//!
//! | Event | Fires when |
//! |---|---|
//! | `inbox_item` | an agent's `user_inbox_add` saved an item |
//! | `agent_failed` | an agent entered the `failed` state |
//! | `outbound_unreachable` | a task sent to a remote agent passed the tracker's unreachable threshold, once per streak |
//! | `reply_while_away` | a main turn the user was part of ended with a reply while no client had the agent's socket open |

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use tokio::sync::broadcast::{self, error::RecvError};
use tokio::task::JoinHandle;

use super::service::PushService;
use super::types::{PushEvent, PushMessage, PushPayload};
use crate::a2a::TrackedTask;
use crate::hub::agent_watch::{AgentChange, AgentChangeKind, AgentChangeReceiver};
use crate::hub::directory::AgentDirectory;
use crate::hub::inbox;
use crate::hub::overview::plain_preview;
use crate::hub::types::{AgentErrorKind, AgentState, AgentSummary, HubEvent};
use crate::memory::types::Visibility;

/// The title of an inbox item push whose item couldn't be read, or has none.
const UNREADABLE_ITEM_TITLE: &str = "New inbox item";

/// What [`PushTriggers`] reads and writes.
pub(crate) struct TriggerInputs {
    /// Sends the pushes.
    pub push: Arc<PushService>,
    /// Where the agents' inboxes and timezone are read from.
    pub directory: Arc<dyn AgentDirectory>,
    /// The hub bus, for agents entering the failed state.
    pub hub_events: broadcast::Receiver<HubEvent>,
    /// The feed of agent changes, for everything else.
    pub changes: AgentChangeReceiver,
    /// Shows the user a notice about how delivery is going.
    pub notice: Box<dyn Fn(String) + Send + Sync>,
}

/// A running set of triggers. It stops when dropped.
pub(crate) struct PushTriggers {
    task: JoinHandle<()>,
}

impl PushTriggers {
    /// Send pushes for what arrives on the inputs' receivers, and pass the
    /// push service's notices to the user.
    ///
    /// Both receivers must be subscribed before anything they should hear
    /// about happens: neither replays. Subscribe before the agents start.
    pub(crate) fn spawn(inputs: TriggerInputs) -> Self {
        Self {
            task: crate::util::spawn_monitored("push-triggers", run(inputs)),
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
    } = inputs;
    let mut notices = push.subscribe_notices();
    let sender = Sender { push, directory };
    let mut rules = Rules::default();
    loop {
        let triggers = tokio::select! {
            event = hub_events.recv() => match event {
                Ok(event) => rules.on_hub_event(&event).into_iter().collect(),
                Err(RecvError::Lagged(missed)) => {
                    tracing::warn!(missed, "push notifications fell behind the hub's events; reading every agent's state again");
                    rules.reconcile(&sender.directory.list())
                }
                Err(RecvError::Closed) => break,
            },
            change = changes.recv() => match change {
                Some(change) => rules.on_change(change).into_iter().collect(),
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
                Vec::new()
            }
        };
        for trigger in triggers {
            sender.send(trigger).await;
        }
    }
    tracing::debug!("push triggers stopped");
}

/// Something worth a notification, before it is worded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Trigger {
    /// An agent saved an item in the user inbox.
    InboxItem { agent: String, item_id: String },
    /// An agent entered the failed state.
    AgentFailed {
        agent: String,
        kind: AgentErrorKind,
        /// Whether it was running, so it stopped by itself, rather than
        /// failing to start.
        was_running: bool,
    },
    /// A task `agent` sent to a remote agent has been unreachable past the
    /// tracker's notice threshold.
    OutboundUnreachable {
        agent: String,
        remote: String,
        task_id: String,
        since: DateTime<Utc>,
    },
    /// `agent` replied to the user while no client was connected.
    ReplyWhileAway { agent: String, reply: String },
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

/// The state the rules need between events.
#[derive(Default)]
struct Rules {
    /// The state each agent was last seen in, to tell entering `failed` from
    /// staying there. An agent never seen is stopped.
    states: BTreeMap<String, AgentState>,
    /// The `(agent, task id)` of each outbound task whose current
    /// unreachable streak has been pushed.
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

    /// Take a fresh list of the agents after hub events were lost: one that
    /// entered `failed` meanwhile still gets its push.
    fn reconcile(&mut self, agents: &[AgentSummary]) -> Vec<Trigger> {
        self.states
            .retain(|name, _| agents.iter().any(|agent| &agent.name == name));
        agents
            .iter()
            .filter_map(|agent| self.observe(agent))
            .collect()
    }

    /// Note the state `agent` is in. Entering `failed` is a trigger, whichever
    /// event reported it: a created agent whose start failed is reported by
    /// its state change and again by its creation, and only the first counts.
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
            // The save also arrives as a file change, which is not an
            // addition: only the tool's own announcement counts.
            AgentChangeKind::UserInboxAdded { item_id } => {
                Some(Trigger::InboxItem { agent, item_id })
            }
            AgentChangeKind::TurnEnded(turn) => {
                // A background turn's reply is not for the user, and a
                // connected client already shows the reply.
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
            | AgentChangeKind::WatchedPathChanged(_) => None,
        }
    }

    /// The tracker announces a task at the start of an unreachable streak,
    /// when the streak passes its threshold (`unreachable_notified` turns
    /// true), and when it ends. Only the second is a trigger, and remembering
    /// that it was pushed keeps any later announcement of the same streak
    /// from sending another.
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

/// Words a trigger and sends it.
struct Sender {
    push: Arc<PushService>,
    directory: Arc<dyn AgentDirectory>,
}

impl Sender {
    async fn send(&self, trigger: Trigger) {
        let event = trigger.event();
        let skip = self.push.presence().active_devices();
        // Wording a push can read every agent's inbox, so a push no device
        // would receive is not worded.
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
                let text = match inbox::active_text(self.directory.as_ref(), &agent, &item_id).await
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

    /// The total of unread inbox items, which every push sets on the app
    /// badge. A count that can't be made still lets the push go out, with 0.
    async fn badge(&self) -> u32 {
        match inbox::unread(self.directory.as_ref()).await {
            Ok(unread) => unread.total,
            Err(e) => {
                tracing::warn!(error = %e, "couldn't count unread inbox items for a push's badge, so it carries 0");
                0
            }
        }
    }

    /// `at` as the hub's timezone reads it: the time of day, with the date
    /// when it isn't today.
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

/// `at` as the time of day, and the date too when it isn't `now`'s.
fn clock_text(at: DateTime<chrono_tz::Tz>, now: DateTime<chrono_tz::Tz>) -> String {
    if at.date_naive() == now.date_naive() {
        at.format("%H:%M").to_string()
    } else {
        at.format("%b %-d, %H:%M").to_string()
    }
}

/// `value` escaped for a URL query.
fn query_escape(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
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
        format!(
            "/inbox?item={}:{}",
            query_escape(agent),
            query_escape(item_id)
        ),
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
