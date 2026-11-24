use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::sync::{Mutex, Notify, broadcast};
use tokio::time::Instant;

use super::preview::plain_preview;
use super::types::{
    AgentOverview, LastMessage, LastMessageRole, LiveSession, OverviewResponse, TimePrecision,
};
use super::{disk, outbound, upcoming};
use crate::AgentDirectory;
use crate::agent_watch::MainTurnEnded;
use crate::mail::count_unread;
use residuum_background::team::AgentState;
use residuum_core::time::format_rfc3339;
use residuum_memory::types::Visibility;

pub const COALESCE_WINDOW: Duration = Duration::from_secs(1);

const FRAME_CAPACITY: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Part {
    LastMessage,
    Sessions,
    Inbox,
    Upcoming,
    OutboundProblems,
}

impl Part {
    pub(super) const ALL: [Self; 5] = [
        Self::LastMessage,
        Self::Sessions,
        Self::Inbox,
        Self::Upcoming,
        Self::OutboundProblems,
    ];

    const WHEN_REQUESTED_RUNNING: [Self; 2] = [Self::Sessions, Self::Inbox];
}

const NOTICE_SLACK: Duration = Duration::from_millis(5);

struct Tracked {
    overview: AgentOverview,
    stale: BTreeSet<Part>,
    reported: Option<LastMessage>,
    sent: Option<AgentOverview>,
    due: Option<Instant>,
    notice_at: Option<Instant>,
    unreadable: BTreeMap<Part, Vec<String>>,
}

impl Tracked {
    fn new(name: &str) -> Self {
        Self {
            overview: AgentOverview::empty(name),
            stale: Part::ALL.into_iter().collect(),
            reported: None,
            sent: None,
            due: None,
            notice_at: None,
            unreadable: BTreeMap::new(),
        }
    }

    fn report_unreadable(&mut self, agent: &str, part: Part, problems: Vec<String>) {
        let before = self.unreadable.get(&part).map_or(&[][..], Vec::as_slice);
        if before == problems.as_slice() {
            return;
        }
        for problem in problems.iter().filter(|problem| !before.contains(problem)) {
            tracing::warn!(agent = %agent, part = ?part, "{problem}");
        }
        if problems.is_empty() {
            tracing::info!(agent = %agent, part = ?part, "an agent's overview can read what it couldn't before");
            self.unreadable.remove(&part);
        } else {
            self.unreadable.insert(part, problems);
        }
    }
}

pub struct TeamOverview {
    directory: Arc<dyn AgentDirectory>,
    boot_id: String,
    window: Duration,
    agents: Mutex<BTreeMap<String, Tracked>>,
    frames: broadcast::Sender<AgentOverview>,
    due_changed: Notify,
}

impl TeamOverview {
    #[must_use]
    pub fn new(boot_id: impl Into<String>, directory: Arc<dyn AgentDirectory>) -> Arc<Self> {
        Self::with_window(boot_id, directory, COALESCE_WINDOW)
    }

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

    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<AgentOverview> {
        self.frames.subscribe()
    }

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

    pub async fn inbox_changed(&self, agent: &str) {
        self.changed(agent, &[Part::Inbox]).await;
    }

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

    pub(super) async fn announce(&self, agent: &str) {
        let mut agents = self.agents.lock().await;
        agents.insert(agent.to_string(), Tracked::new(agent));
        self.send_now(&mut agents, agent).await;
    }

    pub(super) async fn forget(&self, agent: &str) {
        self.agents.lock().await.remove(agent);
    }

    pub(crate) async fn next_due(&self) -> Option<Instant> {
        self.agents
            .lock()
            .await
            .values()
            .filter_map(|tracked| match (tracked.due, tracked.notice_at) {
                (Some(due), Some(notice)) => Some(due.min(notice)),
                (due, notice) => due.or(notice),
            })
            .min()
    }

    pub(super) async fn due_changed(&self) {
        self.due_changed.notified().await;
    }

    pub(super) async fn send_due(&self) {
        let mut agents = self.agents.lock().await;
        let now = Instant::now();
        for tracked in agents.values_mut() {
            if tracked.notice_at.is_some_and(|at| at <= now) {
                tracked.notice_at = None;
                tracked.stale.insert(Part::OutboundProblems);
                tracked.due.get_or_insert(now + self.window);
            }
        }
        let due: Vec<String> = agents
            .iter()
            .filter(|(_, tracked)| tracked.due.is_some_and(|at| at <= now))
            .map(|(name, _)| name.clone())
            .collect();
        for name in due {
            self.send_now(&mut agents, &name).await;
        }
    }

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
            self.frames.send(tracked.overview.clone()).ok();
        }
    }

    fn is_running(&self, agent: &str) -> Option<bool> {
        self.directory
            .summary(agent)
            .ok()
            .map(|summary| summary.state == AgentState::Running)
    }

    async fn refresh(&self, agent: &str, tracked: &mut Tracked, parts: BTreeSet<Part>) -> bool {
        let (Some(running), Ok(files)) =
            (self.is_running(agent), self.directory.agent_files(agent))
        else {
            tracing::debug!(agent = %agent, "an agent was deleted while its overview was read; dropping it");
            return false;
        };
        if !running {
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
                Part::Upcoming => {
                    let read = upcoming::read(agent, &files, Utc::now()).await;
                    tracked.overview.upcoming = read.runs;
                    tracked.report_unreadable(agent, part, read.problems);
                }
                Part::OutboundProblems => {
                    let now = Utc::now();
                    let read = if running {
                        outbound::read(&files, now).await
                    } else {
                        outbound::OutboundRead::none()
                    };
                    tracked.overview.outbound_problems = read.problems;
                    tracked.notice_at = read.next_notice_at.map(|at| instant_at(at, now));
                    if tracked.notice_at.is_some() {
                        self.due_changed.notify_one();
                    }
                    tracked.report_unreadable(agent, part, read.problem.into_iter().collect());
                }
            }
        }
        true
    }
}

fn instant_at(at: DateTime<Utc>, now: DateTime<Utc>) -> Instant {
    Instant::now() + (at - now).to_std().unwrap_or(Duration::ZERO) + NOTICE_SLACK
}

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
