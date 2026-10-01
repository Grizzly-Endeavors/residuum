//! [`TeamOverview`]: every agent's [`AgentOverview`], kept current and told to
//! clients as it changes.
//!
//! The overview is read from the places that hold the truth: an agent's files
//! (its inbox, its history), its session registry, and what the activity
//! tracker's turn hook reported. A change that arrives says which **part** of
//! an agent's overview to read again ([`Part`]); the new value is read when
//! the agent's frame goes out, so a burst of changes costs one read and one
//! frame.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, Notify, broadcast};
use tokio::time::Instant;

use super::disk;
use super::preview::plain_preview;
use super::types::{
    AgentOverview, LastMessage, LastMessageRole, LiveSession, OverviewResponse, TimePrecision,
};
use crate::hub::AgentDirectory;
use crate::hub::agent_watch::MainTurnEnded;
use crate::hub::inbox::count_unread;
use crate::hub::types::AgentState;
use crate::memory::types::Visibility;
use crate::time::format_rfc3339;

/// How long an agent's changes are gathered before its overview is sent. The
/// first change after a frame starts the wait, and the frame that ends it
/// shows the agent as it is then, so an agent gets at most one frame per
/// window and the last state is always sent.
pub const COALESCE_WINDOW: Duration = Duration::from_secs(1);

/// How many frames a subscriber can fall behind by before it is told it lost
/// some.
const FRAME_CAPACITY: usize = 256;

/// One piece of an agent's overview, read from its own source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Part {
    /// `last_message`: the turn hook's report while the agent runs, its
    /// files otherwise.
    LastMessage,
    /// `live_sessions`: the agent's session registry.
    Sessions,
    /// `inbox_unread`: the agent's user inbox files.
    Inbox,
}

impl Part {
    /// Every part.
    pub(super) const ALL: [Self; 3] = [Self::LastMessage, Self::Sessions, Self::Inbox];

    /// The parts that are read again whenever an overview is requested of a
    /// running agent. The turn hook keeps the rest current.
    const WHEN_REQUESTED_RUNNING: [Self; 2] = [Self::Sessions, Self::Inbox];
}

/// What the service keeps about one agent.
struct Tracked {
    /// The agent as last read.
    overview: AgentOverview,
    /// The parts of `overview` that may be out of date.
    stale: BTreeSet<Part>,
    /// The newest turn with a message for the user that the activity
    /// tracker's turn hook reported while the agent ran.
    reported: Option<LastMessage>,
    /// What clients last learned: the last frame sent, or the answer to the
    /// request that first found the agent. `None` until then.
    sent: Option<AgentOverview>,
    /// When the next frame goes out, while there is a change to send.
    due: Option<Instant>,
}

impl Tracked {
    fn new(name: &str) -> Self {
        Self {
            overview: AgentOverview::empty(name),
            stale: Part::ALL.into_iter().collect(),
            reported: None,
            sent: None,
            due: None,
        }
    }
}

/// The overview of every agent the hub hosts.
pub struct TeamOverview {
    directory: Arc<dyn AgentDirectory>,
    boot_id: String,
    window: Duration,
    agents: Mutex<BTreeMap<String, Tracked>>,
    frames: broadcast::Sender<AgentOverview>,
    /// Woken when a frame becomes due, so the tracker's wait can end sooner.
    due_changed: Notify,
}

impl TeamOverview {
    /// An overview of the agents in `directory`, which names its hub process
    /// `boot_id`, sending frames with the [`COALESCE_WINDOW`].
    #[must_use]
    pub fn new(boot_id: impl Into<String>, directory: Arc<dyn AgentDirectory>) -> Arc<Self> {
        Self::with_window(boot_id, directory, COALESCE_WINDOW)
    }

    /// As [`Self::new`], with frames gathered for `window` instead.
    #[must_use]
    pub fn with_window(
        boot_id: impl Into<String>,
        directory: Arc<dyn AgentDirectory>,
        window: Duration,
    ) -> Arc<Self> {
        let (frames, _first_subscriber) = broadcast::channel(FRAME_CAPACITY);
        Arc::new(Self {
            directory,
            boot_id: boot_id.into(),
            window,
            agents: Mutex::new(BTreeMap::new()),
            frames,
            due_changed: Notify::new(),
        })
    }

    /// Subscribe to the frames: an agent's whole overview each time any of it
    /// changes. A receiver that falls behind is told how many it missed.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<AgentOverview> {
        self.frames.subscribe()
    }

    /// Every agent's overview, sorted by name.
    ///
    /// The user inbox of every agent is counted again, and a stopped agent
    /// is read from its files entirely. When that finds something clients
    /// weren't told, their next frame says it.
    pub async fn snapshot(&self) -> OverviewResponse {
        let mut names: Vec<String> = self
            .directory
            .list()
            .into_iter()
            .map(|summary| summary.name)
            .collect();
        names.sort();

        let mut agents = self.agents.lock().await;
        agents.retain(|name, _| names.contains(name));
        let mut overviews = Vec::with_capacity(names.len());
        let mut gone = Vec::new();
        let mut frame_due = false;
        for name in &names {
            let known = agents.contains_key(name);
            let tracked = agents
                .entry(name.clone())
                .or_insert_with(|| Tracked::new(name));
            let mut parts = std::mem::take(&mut tracked.stale);
            match self.is_running(name) {
                Some(true) => parts.extend(Part::WHEN_REQUESTED_RUNNING),
                Some(false) => parts.extend(Part::ALL),
                None => {}
            }
            if !self.refresh(name, tracked, parts).await {
                gone.push(name.clone());
                continue;
            }
            overviews.push(tracked.overview.clone());
            if !known {
                tracked.sent = Some(tracked.overview.clone());
            } else if tracked.sent.as_ref() != Some(&tracked.overview) {
                tracked.due.get_or_insert(Instant::now() + self.window);
                frame_due = true;
            }
        }
        for name in gone {
            agents.remove(&name);
        }
        drop(agents);
        if frame_due {
            self.due_changed.notify_one();
        }
        OverviewResponse {
            boot_id: self.boot_id.clone(),
            agents: overviews,
        }
    }

    /// The hub changed the agent's user inbox (read, archived or restored an
    /// item), so its unread count is read again.
    pub async fn inbox_changed(&self, agent: &str) {
        self.changed(agent, &[Part::Inbox]).await;
    }

    /// `parts` of `agent`'s overview may be out of date. Its frame goes out
    /// when its window ends.
    pub(super) async fn changed(&self, agent: &str, parts: &[Part]) {
        if parts.is_empty() {
            return;
        }
        {
            let mut agents = self.agents.lock().await;
            let tracked = agents
                .entry(agent.to_string())
                .or_insert_with(|| Tracked::new(agent));
            tracked.stale.extend(parts.iter().copied());
            tracked.due.get_or_insert(Instant::now() + self.window);
        }
        self.due_changed.notify_one();
    }

    /// Everything about every agent may be out of date, because changes
    /// were missed.
    pub(super) async fn changed_everywhere(&self) {
        let names: BTreeSet<String> = self
            .directory
            .list()
            .into_iter()
            .map(|summary| summary.name)
            .collect();
        {
            let mut agents = self.agents.lock().await;
            agents.retain(|name, _| names.contains(name));
            for name in names {
                let tracked = agents
                    .entry(name.clone())
                    .or_insert_with(|| Tracked::new(&name));
                tracked.stale.extend(Part::ALL);
                tracked.due.get_or_insert(Instant::now() + self.window);
            }
        }
        self.due_changed.notify_one();
    }

    /// A main turn of `agent` ended. A turn the user was part of becomes its
    /// last message: the reply when it had one, else what the user said. A
    /// background turn is not part of the user's conversation and changes
    /// nothing.
    pub(super) async fn turn_ended(&self, agent: &str, turn: &MainTurnEnded) {
        if turn.visibility != Visibility::User {
            return;
        }
        let Ok(files) = self.directory.agent_files(agent) else {
            tracing::debug!(agent = %agent, "an agent's turn ended after it was gone; its last message isn't updated");
            return;
        };
        let Some(message) = reported_message(turn, files.timezone) else {
            return;
        };
        {
            let mut agents = self.agents.lock().await;
            let tracked = agents
                .entry(agent.to_string())
                .or_insert_with(|| Tracked::new(agent));
            tracked.reported = Some(message);
            tracked.stale.insert(Part::LastMessage);
            tracked.due.get_or_insert(Instant::now() + self.window);
        }
        self.due_changed.notify_one();
    }

    /// A new agent appeared (created or restored): its overview goes out
    /// now, without waiting for a window.
    pub(super) async fn announce(&self, agent: &str) {
        let mut agents = self.agents.lock().await;
        agents.insert(agent.to_string(), Tracked::new(agent));
        self.send_now(&mut agents, agent).await;
    }

    /// The agent is deleted: nothing more is said about it.
    pub(super) async fn forget(&self, agent: &str) {
        self.agents.lock().await.remove(agent);
    }

    /// When the soonest frame is due, if any is.
    pub(super) async fn next_due(&self) -> Option<Instant> {
        self.agents
            .lock()
            .await
            .values()
            .filter_map(|tracked| tracked.due)
            .min()
    }

    /// Resolves when a frame becomes due that [`Self::next_due`] didn't know of.
    pub(super) async fn due_changed(&self) {
        self.due_changed.notified().await;
    }

    /// Send the frame of every agent whose window has ended.
    pub(super) async fn send_due(&self) {
        let mut agents = self.agents.lock().await;
        let now = Instant::now();
        let due: Vec<String> = agents
            .iter()
            .filter(|(_, tracked)| tracked.due.is_some_and(|at| at <= now))
            .map(|(name, _)| name.clone())
            .collect();
        for name in due {
            self.send_now(&mut agents, &name).await;
        }
    }

    /// Read what is out of date about `agent` and send its overview if clients
    /// don't have it.
    async fn send_now(&self, agents: &mut BTreeMap<String, Tracked>, agent: &str) {
        let Some(tracked) = agents.get_mut(agent) else {
            return;
        };
        tracked.due = None;
        let stale = std::mem::take(&mut tracked.stale);
        if !self.refresh(agent, tracked, stale).await {
            agents.remove(agent);
            return;
        }
        if tracked.sent.as_ref() != Some(&tracked.overview) {
            tracked.sent = Some(tracked.overview.clone());
            // No subscribers is the normal state until a client opens the
            // hub WebSocket.
            self.frames.send(tracked.overview.clone()).ok();
        }
    }

    /// Whether the agent is running, or `None` when the hub has no such agent.
    fn is_running(&self, agent: &str) -> Option<bool> {
        self.directory
            .summary(agent)
            .ok()
            .map(|summary| summary.state == AgentState::Running)
    }

    /// Read `parts` of the agent's overview from where each is kept. `false`
    /// when the agent is gone, so there is nothing to keep.
    async fn refresh(&self, agent: &str, tracked: &mut Tracked, parts: BTreeSet<Part>) -> bool {
        let (Some(running), Ok(files)) =
            (self.is_running(agent), self.directory.agent_files(agent))
        else {
            tracing::debug!(agent = %agent, "an agent was deleted while its overview was read; dropping it");
            return false;
        };
        if !running {
            // What a past run reported is on disk now.
            tracked.reported = None;
        }
        for part in parts {
            match part {
                Part::LastMessage => {
                    tracked.overview.last_message = match &tracked.reported {
                        Some(reported) => Some(reported.clone()),
                        None => disk::last_message(agent, &files).await,
                    };
                }
                Part::Sessions => {
                    tracked.overview.live_sessions = if running {
                        self.directory
                            .live_sessions(agent)
                            .iter()
                            .map(LiveSession::of)
                            .collect()
                    } else {
                        Vec::new()
                    };
                }
                Part::Inbox => {
                    tracked.overview.inbox_unread = count_unread(agent, &files.dir).await;
                }
            }
        }
        true
    }
}

/// The last message `turn` leaves the user: the reply, or when the reply has
/// nothing to show, what the user said.
fn reported_message(turn: &MainTurnEnded, timezone: chrono_tz::Tz) -> Option<LastMessage> {
    let at = format_rfc3339(&turn.at.with_timezone(&timezone));
    [
        (LastMessageRole::Assistant, turn.reply.as_deref()),
        (LastMessageRole::User, turn.user_message.as_deref()),
    ]
    .into_iter()
    .find_map(|(role, text)| {
        let preview = plain_preview(text?);
        (!preview.is_empty()).then(|| LastMessage {
            role,
            preview,
            at: at.clone(),
            at_precision: TimePrecision::Minute,
        })
    })
}
