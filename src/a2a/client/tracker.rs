//! Remote task tracker: persists outbound A2A tasks this instance started on
//! other agents, watches them (streaming or polling, depending on what the
//! agent's card declares) until they need the sender's attention, and
//! delivers the outcome back through `AgentMessenger`. See
//! `docs/systems-usage/a2a.md`.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use futures_util::StreamExt as _;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::background::messaging::{AgentMessenger, DeliveryOutcome};
use crate::background::registry::MAIN_ADDRESS;
use crate::bus::{
    NoticeEvent, NotifyName, OutboundA2aTaskEvent, SYSTEM_CHANNEL, SessionAddress, topics,
};
use crate::interfaces::attachment::{self, AttachmentInfo};

use super::hub::{A2aClientHub, HubError, task_state_str};

/// How long an agent must stay unreachable before the sender gets a single
/// notice; retries continue either way.
const UNREACHABLE_NOTICE_AFTER: chrono::Duration = chrono::Duration::minutes(10);
/// How long a completed task's record is kept before being pruned on load.
const PRUNE_TERMINAL_AFTER: chrono::Duration = chrono::Duration::days(30);
/// Poll/reconnect backoff bounds, matching the plan's 5s→60s.
const MIN_BACKOFF: Duration = Duration::from_secs(5);
const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// Text parts at or under this size are delivered inline; larger ones (and
/// every file part) are saved to the agent inbox instead.
const INLINE_TEXT_LIMIT: usize = 4096;
/// Appended to what the sending agent is told when the user stops one of
/// its tasks from the web UI, so it doesn't take the cancel for the remote
/// agent's own doing and retry.
const USER_STOPPED_NOTE: &str = "Stopped by the user from the web UI.";

/// One tracked outbound task, persisted at `{workspace}/a2a/outbound.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackedTask {
    pub sender_address: String,
    pub agent: String,
    pub task_id: String,
    pub context_id: String,
    /// A plain word from [`task_state_str`] — `"working"`, `"input_required"`,
    /// `"completed"`, etc.
    pub state: String,
    #[serde(default)]
    pub last_status_text: Option<String>,
    pub hop_count: u32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// When this task's agent was first found unreachable in the current
    /// unreachable streak; cleared on the next successful contact.
    #[serde(default)]
    pub first_unreachable_at: Option<DateTime<Utc>>,
    /// Whether the unreachable notice (sent once a streak passes
    /// [`UNREACHABLE_NOTICE_AFTER`]) has already fired for the current
    /// streak.
    #[serde(default)]
    pub unreachable_notified: bool,
    /// Whether a delivery has already fired for the current turn (since the
    /// last [`RemoteTaskTracker::track`] call) — a direct cancel and the
    /// background watcher can both observe the same terminal event for one
    /// turn, and only the first should reach the sender. Reset to `false`
    /// every time `track` records a new send or follow-up.
    #[serde(default)]
    pub notified_this_turn: bool,
    /// Set when the user stopped watching this task because its agent
    /// couldn't be reached to cancel it (see
    /// [`RemoteTaskTracker::stop_watching`]). A late update from the agent
    /// is ignored rather than reopening a task the user closed.
    #[serde(default)]
    pub stopped_by_user: bool,
}

impl TrackedTask {
    /// Not in a terminal state — still worth watching (or resuming a watch
    /// for, e.g. after a follow-up).
    #[must_use]
    pub fn is_open(&self) -> bool {
        !matches!(
            self.state.as_str(),
            "completed" | "failed" | "canceled" | "rejected"
        )
    }

    /// Waiting on the sender before it can make progress.
    #[must_use]
    pub fn awaits_reply(&self) -> bool {
        matches!(self.state.as_str(), "input_required" | "auth_required")
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct OutboundStore {
    /// Keyed by task id.
    #[serde(default)]
    tasks: HashMap<String, TrackedTask>,
    /// sender address → agent name → context id, so a conversation with an
    /// agent continues across tasks.
    #[serde(default)]
    contexts: HashMap<String, HashMap<String, String>>,
}

/// Watches and reports on outbound A2A tasks, persisting state at
/// `{workspace}/a2a/outbound.json`.
pub struct RemoteTaskTracker {
    path: PathBuf,
    store: RwLock<OutboundStore>,
    hub: Arc<A2aClientHub>,
    messenger: Arc<AgentMessenger>,
    /// Where inbound-style attachments (large text parts, file parts) from a
    /// remote agent's artifacts are saved.
    inbox_dir: PathBuf,
}

impl RemoteTaskTracker {
    /// Load persisted tasks from `path`. A missing or unreadable file starts
    /// empty and logs a warning — a tracker failure must never block
    /// startup. Terminal tasks older than 30 days are pruned on load.
    #[must_use]
    pub async fn load(
        path: PathBuf,
        hub: Arc<A2aClientHub>,
        messenger: Arc<AgentMessenger>,
        inbox_dir: PathBuf,
    ) -> Arc<Self> {
        let mut store = match tokio::fs::read_to_string(&path).await {
            Ok(raw) => match serde_json::from_str::<OutboundStore>(&raw) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        path = %path.display(),
                        "failed to parse a2a outbound tasks, starting empty"
                    );
                    OutboundStore::default()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => OutboundStore::default(),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    path = %path.display(),
                    "failed to read a2a outbound tasks, starting empty"
                );
                OutboundStore::default()
            }
        };

        let now = Utc::now();
        let before = store.tasks.len();
        store
            .tasks
            .retain(|_, t| t.is_open() || now - t.updated_at < PRUNE_TERMINAL_AFTER);
        if store.tasks.len() != before {
            tracing::debug!(
                pruned = before - store.tasks.len(),
                "pruned old a2a outbound tasks"
            );
        }

        Arc::new(Self {
            path,
            store: RwLock::new(store),
            hub,
            messenger,
            inbox_dir,
        })
    }

    async fn persist(&self, snapshot: &OutboundStore) {
        match serde_json::to_vec_pretty(snapshot) {
            Ok(bytes) => {
                if let Err(e) = crate::util::fs::atomic_write(&self.path, bytes).await {
                    tracing::error!(error = %e, path = %self.path.display(), "failed to persist a2a outbound tasks");
                }
            }
            Err(e) => tracing::error!(error = %e, "failed to serialize a2a outbound tasks"),
        }
    }

    /// Resume watching every open (non-terminal) task. Call once at startup
    /// (after [`load`](Self::load)) and after every reload that might affect
    /// reachability.
    pub async fn spawn_resume_watchers(self: &Arc<Self>) {
        let task_ids: Vec<String> = {
            let store = self.store.read().await;
            store
                .tasks
                .values()
                .filter(|t| t.is_open())
                .map(|t| t.task_id.clone())
                .collect()
        };
        for task_id in task_ids {
            self.spawn_watch(task_id);
        }
    }

    fn spawn_watch(self: &Arc<Self>, task_id: String) {
        let tracker = Arc::clone(self);
        crate::util::spawn_monitored("a2a-task-watch", async move {
            tracker.watch_loop(task_id).await;
        });
    }

    /// The sender's currently open (`INPUT_REQUIRED`/`AUTH_REQUIRED`) task
    /// with `agent`, if any — a follow-up should go to this task.
    pub async fn awaiting_reply_task_for(&self, sender: &str, agent: &str) -> Option<TrackedTask> {
        let store = self.store.read().await;
        store
            .tasks
            .values()
            .find(|t| t.sender_address == sender && t.agent == agent && t.awaits_reply())
            .cloned()
    }

    /// Any of the sender's still-open tasks with `agent` — used by
    /// `stop_agent`, which can cancel a task that's merely `working`, not
    /// only one awaiting a reply.
    pub async fn any_open_task_for(&self, sender: &str, agent: &str) -> Option<TrackedTask> {
        let store = self.store.read().await;
        store
            .tasks
            .values()
            .find(|t| t.sender_address == sender && t.agent == agent && t.is_open())
            .cloned()
    }

    /// The persisted (sender, agent) context id, if any task has ever been
    /// started between them.
    pub async fn context_for(&self, sender: &str, agent: &str) -> Option<String> {
        self.store
            .read()
            .await
            .contexts
            .get(sender)
            .and_then(|m| m.get(agent))
            .cloned()
    }

    /// Every open task belonging to `sender`, oldest first.
    pub async fn open_tasks_for(&self, sender: &str) -> Vec<TrackedTask> {
        let store = self.store.read().await;
        let mut tasks: Vec<TrackedTask> = store
            .tasks
            .values()
            .filter(|t| t.sender_address == sender && t.is_open())
            .cloned()
            .collect();
        tasks.sort_by_key(|t| t.updated_at);
        tasks
    }

    /// Record a freshly-sent task (new or follow-up) and (re)start watching
    /// it.
    pub async fn track(
        self: &Arc<Self>,
        sender: &SessionAddress,
        agent: &str,
        task_id: String,
        context_id: String,
        state: &str,
        hop_count: u32,
    ) {
        let sender_str = sender.as_ref().to_string();
        let now = Utc::now();
        let snapshot = {
            let mut store = self.store.write().await;
            store
                .contexts
                .entry(sender_str.clone())
                .or_default()
                .insert(agent.to_string(), context_id.clone());
            let entry = store
                .tasks
                .entry(task_id.clone())
                .or_insert_with(|| TrackedTask {
                    sender_address: sender_str.clone(),
                    agent: agent.to_string(),
                    task_id: task_id.clone(),
                    context_id: context_id.clone(),
                    state: state.to_string(),
                    last_status_text: None,
                    hop_count,
                    created_at: now,
                    updated_at: now,
                    first_unreachable_at: None,
                    unreachable_notified: false,
                    notified_this_turn: false,
                    stopped_by_user: false,
                });
            entry.state = state.to_string();
            entry.hop_count = hop_count;
            entry.context_id = context_id;
            entry.updated_at = now;
            entry.first_unreachable_at = None;
            entry.unreachable_notified = false;
            entry.notified_this_turn = false;
            entry.stopped_by_user = false;
            (entry.clone(), store.clone())
        };
        let (task, snapshot) = snapshot;
        self.persist(&snapshot).await;
        self.publish_task_change(&task).await;
        self.spawn_watch(task_id);
    }

    /// Every open task, from every sender, newest first — the web sessions
    /// sidebar's list of tasks sent to other agents.
    pub async fn open_tasks(&self) -> Vec<TrackedTask> {
        let store = self.store.read().await;
        let mut tasks: Vec<TrackedTask> = store
            .tasks
            .values()
            .filter(|t| t.is_open())
            .cloned()
            .collect();
        tasks.sort_by_key(|t| std::cmp::Reverse(t.created_at));
        tasks
    }

    /// Cancel the open task `task_id` on its remote agent — the web sessions
    /// sidebar's Stop button. The sending agent is told the task was
    /// canceled and that the user stopped it.
    ///
    /// Returns `Ok(None)` when no open task has that id.
    ///
    /// # Errors
    /// Returns [`HubError`] if the agent can't currently be reached to
    /// cancel it; [`Self::stop_watching`] is the way out of that.
    pub async fn stop_task(&self, task_id: &str) -> Result<Option<TrackedTask>, HubError> {
        let Some(task) = self.get(task_id).await.filter(TrackedTask::is_open) else {
            return Ok(None);
        };
        self.cancel_remote(&task, Some(USER_STOPPED_NOTE)).await?;
        Ok(self.get(task_id).await)
    }

    /// Close the open task `task_id` locally without reaching its agent —
    /// for a task whose agent is unreachable, so the user can end the
    /// retries (and their notices) anyway. The task may still be running
    /// on the remote side; the sending agent is told so.
    ///
    /// Returns `None` when no open task has that id.
    pub async fn stop_watching(&self, task_id: &str) -> Option<TrackedTask> {
        let (task, snapshot) = {
            let mut store = self.store.write().await;
            let entry = store.tasks.get_mut(task_id).filter(|t| t.is_open())?;
            entry.state = "canceled".to_string();
            entry.last_status_text = Some(
                "The user stopped watching this task because its agent couldn't be reached to \
                 cancel it. It may still be running on the remote side."
                    .to_string(),
            );
            entry.stopped_by_user = true;
            entry.first_unreachable_at = None;
            entry.unreachable_notified = false;
            entry.notified_this_turn = true;
            entry.updated_at = Utc::now();
            (entry.clone(), store.clone())
        };
        self.persist(&snapshot).await;
        tracing::info!(task_id, agent = %task.agent, "user stopped watching a2a remote task");
        self.publish_task_change(&task).await;
        self.deliver(&task, task.last_status_text.as_deref(), Vec::new(), None)
            .await;
        Some(task)
    }

    /// Cancel the sender's open task with `agent`, if any, returning its
    /// task id.
    ///
    /// # Errors
    /// Returns [`HubError`] if a task exists but the agent can't currently
    /// be reached to cancel it.
    pub async fn cancel_open_task(
        &self,
        sender: &str,
        agent: &str,
    ) -> Result<Option<String>, HubError> {
        let Some(task) = self.any_open_task_for(sender, agent).await else {
            return Ok(None);
        };
        self.cancel_remote(&task, None).await?;
        Ok(Some(task.task_id))
    }

    /// Ask `task`'s agent to cancel it and apply the reply, adding `note`
    /// to what the sending agent is told.
    async fn cancel_remote(&self, task: &TrackedTask, note: Option<&str>) -> Result<(), HubError> {
        let (client, _card) = self.hub.client_for(&task.agent).await?;
        match client
            .cancel_task(&a2a::CancelTaskRequest {
                id: task.task_id.clone(),
                metadata: None,
                tenant: None,
            })
            .await
        {
            Ok(remote_task) => {
                self.apply_remote_task_noted(remote_task, note).await;
                Ok(())
            }
            Err(e) => {
                tracing::warn!(task_id = %task.task_id, agent = %task.agent, error = %e, "failed to cancel a2a remote task");
                Err(HubError::RequestFailed(task.agent.clone(), e.to_string()))
            }
        }
    }

    async fn get(&self, task_id: &str) -> Option<TrackedTask> {
        self.store.read().await.tasks.get(task_id).cloned()
    }

    /// Update a tracked task's state/status text after successful contact,
    /// clearing any unreachable streak. `is_final` marks a state that would
    /// warrant delivering to the sender (input/auth-required or terminal);
    /// the returned `should_deliver` is `true` only the first time such a
    /// state is observed for the current turn, so a direct cancel racing the
    /// background watcher's own observation of the same event delivers
    /// exactly once. Returns `None` if the task is no longer tracked (e.g.
    /// concurrently removed).
    async fn update_state(
        &self,
        task_id: &str,
        state: &str,
        text: Option<String>,
        is_final: bool,
    ) -> Option<(TrackedTask, bool)> {
        let (result, snapshot, streak_ended, sender_was_told, changed) = {
            let mut store = self.store.write().await;
            let entry = store.tasks.get_mut(task_id)?;
            if entry.stopped_by_user {
                return None;
            }
            let before = (entry.state.clone(), entry.last_status_text.clone());
            entry.state = state.to_string();
            if let Some(t) = text {
                entry.last_status_text = Some(t);
            }
            entry.updated_at = Utc::now();
            let streak_ended = entry.first_unreachable_at.take().is_some();
            // Captured before clearing: whether this successful contact
            // ends a streak the sender and user were already told about,
            // so exactly one recovery notice goes out per notified streak.
            let sender_was_told = entry.unreachable_notified;
            entry.unreachable_notified = false;
            let should_deliver = is_final && !entry.notified_this_turn;
            if should_deliver {
                entry.notified_this_turn = true;
            }
            let changed =
                streak_ended || before != (entry.state.clone(), entry.last_status_text.clone());
            let task = entry.clone();
            (
                (task, should_deliver),
                store.clone(),
                streak_ended,
                sender_was_told,
                changed,
            )
        };
        self.persist(&snapshot).await;
        if changed {
            self.publish_task_change(&result.0).await;
        }
        if streak_ended {
            tracing::info!(task_id, agent = %result.0.agent, "a2a remote task poll recovered");
        }
        if sender_was_told {
            self.notify_unreachable_recovered(&result.0).await;
        }
        Some(result)
    }

    /// Record an unreachable attempt against `task_id`. Logs at `warn` once
    /// when the failure streak starts, not on every poll — see
    /// [`Self::update_state`] for the matching single-line recovery log
    /// (polling never gives up; it retries with backoff until it recovers or
    /// the task is cancelled). After the streak passes
    /// [`UNREACHABLE_NOTICE_AFTER`], tells the sending agent (a transcript
    /// note) and publishes a user-facing notice, once per streak;
    /// [`Self::update_state`] sends a matching notice to both on recovery.
    async fn note_unreachable(&self, task_id: &str, reason: &str) {
        let outcome = {
            let mut store = self.store.write().await;
            let Some(entry) = store.tasks.get_mut(task_id) else {
                return;
            };
            let now = Utc::now();
            let is_streak_start = entry.first_unreachable_at.is_none();
            let first = *entry.first_unreachable_at.get_or_insert(now);
            let should_notify =
                !entry.unreachable_notified && now - first >= UNREACHABLE_NOTICE_AFTER;
            if should_notify {
                entry.unreachable_notified = true;
            }
            (
                is_streak_start,
                should_notify,
                entry.sender_address.clone(),
                entry.agent.clone(),
                entry.hop_count,
                store.clone(),
            )
        };
        let (is_streak_start, should_notify, sender, agent, hop_count, snapshot) = outcome;
        self.persist(&snapshot).await;
        if is_streak_start && let Some(task) = snapshot.tasks.get(task_id) {
            self.publish_task_change(task).await;
        }
        if is_streak_start {
            tracing::warn!(task_id, agent = %agent, reason, "a2a remote task poll failing; will keep retrying");
        }
        if should_notify {
            let content = format!(
                "[Remote agent a2a:{agent} — task {task_id}] Still unreachable after 10 minutes \
                 ({reason}). Still retrying in the background."
            );
            self.deliver_to_sender(&sender, &agent, hop_count, content)
                .await;
            self.publish_user_notice(&format!(
                "a2a:{agent} has been unreachable for 10 minutes (task {task_id}): {reason}. \
                 Still retrying in the background."
            ))
            .await;
        }
    }

    /// Tell both the sending agent and the user that a task's agent is
    /// reachable again, mirroring [`Self::note_unreachable`]'s own
    /// delivery shape.
    async fn notify_unreachable_recovered(&self, task: &TrackedTask) {
        let content = format!(
            "[Remote agent a2a:{} — task {}] Reachable again.",
            task.agent, task.task_id
        );
        self.deliver_to_sender(&task.sender_address, &task.agent, task.hop_count, content)
            .await;
        self.publish_user_notice(&format!(
            "a2a:{} is reachable again (task {}).",
            task.agent, task.task_id
        ))
        .await;
    }

    /// Publish a plain-language notice to the system notification channel
    /// (the web UI's toast/notice stream, and every other interface
    /// subscribed to it) — the user-facing half of an unreachable/recovery
    /// notice, alongside the agent-facing transcript note.
    async fn publish_user_notice(&self, message: &str) {
        if let Err(e) = self
            .messenger
            .publisher()
            .publish(
                topics::Notification(NotifyName::from(SYSTEM_CHANNEL)),
                NoticeEvent {
                    message: message.to_string(),
                },
            )
            .await
        {
            tracing::warn!(error = %e, "failed to publish a2a task notice");
        }
    }

    /// Tell the web sessions sidebar a task was recorded or changed state.
    async fn publish_task_change(&self, task: &TrackedTask) {
        if let Err(e) = self
            .messenger
            .publisher()
            .publish(
                topics::Notification(NotifyName::from(SYSTEM_CHANNEL)),
                OutboundA2aTaskEvent { task: task.clone() },
            )
            .await
        {
            tracing::warn!(error = %e, task_id = %task.task_id, "failed to publish a2a task change");
        }
    }

    async fn deliver_to_sender(&self, sender: &str, agent: &str, hop_count: u32, content: String) {
        let from = SessionAddress::from(format!("a2a:{agent}"));
        match self
            .messenger
            .send(
                sender,
                from.clone(),
                "remote".to_string(),
                content.clone(),
                hop_count,
            )
            .await
        {
            Ok(DeliveryOutcome::Unknown) => {
                let note = format!(
                    "[Remote agent a2a:{agent} sent a reply for {sender}, which no longer exists]\n{content}"
                );
                if let Err(e) = self
                    .messenger
                    .send(MAIN_ADDRESS, from, "remote".to_string(), note, hop_count)
                    .await
                {
                    tracing::error!(error = %e, sender, agent, "failed to deliver orphaned a2a remote-task result to main");
                }
            }
            Ok(_) => {}
            Err(e) => {
                tracing::error!(error = %e, sender, agent, "failed to deliver a2a remote-task result");
            }
        }
    }

    /// Apply a fully-fetched `Task` (from `get_task` or `cancel_task`),
    /// delivering to the sender if it reached a state that needs their
    /// attention. Returns whether that happened (the caller should stop
    /// watching).
    async fn apply_remote_task(&self, task: a2a::Task) -> bool {
        self.apply_remote_task_noted(task, None).await
    }

    /// [`Self::apply_remote_task`], adding `note` to the delivery if this
    /// update delivers one.
    async fn apply_remote_task_noted(&self, task: a2a::Task, note: Option<&str>) -> bool {
        let state = task_state_str(&task.status.state);
        let text = task
            .status
            .message
            .as_ref()
            .and_then(a2a::Message::text)
            .map(str::to_string);
        let is_final = task.status.state.is_terminal()
            || task.status.state == a2a::TaskState::InputRequired
            || task.status.state == a2a::TaskState::AuthRequired;
        let Some((updated, should_deliver)) = self
            .update_state(&task.id, state, text.clone(), is_final)
            .await
        else {
            return true;
        };
        if should_deliver {
            self.deliver(
                &updated,
                text.as_deref(),
                task.artifacts.unwrap_or_default(),
                note,
            )
            .await;
        }
        is_final
    }

    async fn watch_loop(self: Arc<Self>, task_id: String) {
        loop {
            let Some(task) = self.get(&task_id).await else {
                return;
            };
            if !task.is_open() {
                return;
            }
            let streams = self
                .hub
                .card_for(&task.agent)
                .await
                .is_some_and(|card| card.capabilities.streaming == Some(true));
            if streams {
                self.watch_via_stream(&task_id).await;
            } else {
                self.watch_via_poll(&task_id).await;
            }
            // Either helper returns only once the task is no longer open (it
            // delivered a final outcome) or the tracked record vanished; the
            // top of the loop re-checks and exits in that case.
        }
    }

    async fn watch_via_poll(self: &Arc<Self>, task_id: &str) {
        let mut backoff = MIN_BACKOFF;
        loop {
            tokio::time::sleep(backoff).await;
            let Some(task) = self.get(task_id).await else {
                return;
            };
            if !task.is_open() {
                return;
            }
            match self.hub.client_for(&task.agent).await {
                Ok((client, _card)) => {
                    let req = a2a::GetTaskRequest {
                        id: task_id.to_string(),
                        history_length: None,
                        tenant: None,
                    };
                    match client.get_task(&req).await {
                        Ok(remote_task) => {
                            backoff = MIN_BACKOFF;
                            if self.apply_remote_task(remote_task).await {
                                return;
                            }
                        }
                        Err(e) => {
                            self.note_unreachable(task_id, &e.to_string()).await;
                            backoff = (backoff * 2).min(MAX_BACKOFF);
                        }
                    }
                }
                Err(e) => {
                    self.note_unreachable(task_id, &e.to_string()).await;
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                }
            }
        }
    }

    /// A single `get_task` check, applying whatever it reports. Returns
    /// whether the caller should stop watching (delivered, or the record
    /// vanished) — the same contract as [`Self::apply_remote_task`]. Used as
    /// a fallback when `subscribe_to_task` can't be used: the reference SDK
    /// server drops a task's live subscription once its executor's own
    /// stream ends, even at a non-terminal state (e.g. `INPUT_REQUIRED`) —
    /// a late subscribe then sees `task_not_found` even though the task
    /// itself is very much still there, just idle. A direct poll sidesteps
    /// that gap.
    async fn poll_once(&self, client: &super::hub::NegotiatedClient, task_id: &str) -> bool {
        let req = a2a::GetTaskRequest {
            id: task_id.to_string(),
            history_length: None,
            tenant: None,
        };
        match client.get_task(&req).await {
            Ok(remote_task) => self.apply_remote_task(remote_task).await,
            Err(e) => {
                self.note_unreachable(task_id, &e.to_string()).await;
                false
            }
        }
    }

    async fn watch_via_stream(self: &Arc<Self>, task_id: &str) {
        loop {
            let Some(task) = self.get(task_id).await else {
                return;
            };
            if !task.is_open() {
                return;
            }
            let (client, _card) = match self.hub.client_for(&task.agent).await {
                Ok(c) => c,
                Err(e) => {
                    self.note_unreachable(task_id, &e.to_string()).await;
                    tokio::time::sleep(MAX_BACKOFF).await;
                    continue;
                }
            };
            let req = a2a::SubscribeToTaskRequest {
                id: task_id.to_string(),
                tenant: None,
            };
            let Ok(mut stream) = client.subscribe_to_task(&req).await else {
                if self.poll_once(&client, task_id).await {
                    return;
                }
                tokio::time::sleep(MIN_BACKOFF).await;
                continue;
            };

            let mut pending_artifacts: Vec<a2a::Artifact> = Vec::new();
            let mut delivered = false;
            while let Some(event) = stream.next().await {
                match event {
                    Ok(a2a::StreamResponse::Task(t)) => {
                        let mut t = t;
                        t.artifacts
                            .get_or_insert_with(Vec::new)
                            .append(&mut pending_artifacts);
                        if self.apply_remote_task(t).await {
                            delivered = true;
                            break;
                        }
                    }
                    Ok(a2a::StreamResponse::StatusUpdate(u)) => {
                        let state = task_state_str(&u.status.state);
                        let text = u
                            .status
                            .message
                            .as_ref()
                            .and_then(a2a::Message::text)
                            .map(str::to_string);
                        let is_final = u.status.state.is_terminal()
                            || u.status.state == a2a::TaskState::InputRequired
                            || u.status.state == a2a::TaskState::AuthRequired;
                        let Some((updated, should_deliver)) = self
                            .update_state(task_id, state, text.clone(), is_final)
                            .await
                        else {
                            return;
                        };
                        if should_deliver {
                            self.deliver(
                                &updated,
                                text.as_deref(),
                                std::mem::take(&mut pending_artifacts),
                                None,
                            )
                            .await;
                        }
                        if is_final {
                            delivered = true;
                            break;
                        }
                    }
                    Ok(a2a::StreamResponse::ArtifactUpdate(a)) => {
                        pending_artifacts.push(a.artifact);
                    }
                    Ok(a2a::StreamResponse::Message(_)) => {
                        // A direct message reply with no task id — nothing
                        // to track here.
                    }
                    Err(e) => {
                        self.note_unreachable(task_id, &e.to_string()).await;
                        break;
                    }
                }
            }
            if delivered {
                return;
            }
            // The stream ended (cleanly or via a mid-stream error) without a
            // final event — most likely the same "execution already ended"
            // gap `subscribe_to_task` itself can hit. Check once via
            // `get_task` before reconnecting.
            if self.poll_once(&client, task_id).await {
                return;
            }
            tokio::time::sleep(MIN_BACKOFF).await;
        }
    }

    async fn deliver(
        &self,
        task: &TrackedTask,
        text: Option<&str>,
        artifacts: Vec<a2a::Artifact>,
        note: Option<&str>,
    ) {
        let mut body = format!(
            "[Remote agent a2a:{agent} — task {task_id}: {state}]\n{text}",
            agent = task.agent,
            task_id = task.task_id,
            state = task.state,
            text = text.unwrap_or("(no status message)"),
        );
        if let Some(note) = note {
            write!(body, "\n{note}").ok();
        }
        for artifact in artifacts {
            self.append_artifact(&mut body, &artifact).await;
        }
        self.deliver_to_sender(&task.sender_address, &task.agent, task.hop_count, body)
            .await;
    }

    async fn append_artifact(&self, body: &mut String, artifact: &a2a::Artifact) {
        for part in &artifact.parts {
            match &part.content {
                a2a::PartContent::Text(text) if text.len() <= INLINE_TEXT_LIMIT => {
                    body.push_str("\n\n");
                    if let Some(name) = &artifact.name {
                        writeln!(body, "[Artifact: {name}]").ok();
                    }
                    body.push_str(text);
                }
                a2a::PartContent::Text(text) => {
                    self.save_bytes_and_note(body, artifact, part, text.clone().into_bytes())
                        .await;
                }
                a2a::PartContent::Raw(bytes) => {
                    self.save_bytes_and_note(body, artifact, part, bytes.clone())
                        .await;
                }
                a2a::PartContent::Url(url) => {
                    self.save_url_and_note(body, artifact, part, url).await;
                }
                a2a::PartContent::Data(value) => {
                    write!(body, "\n\n[Artifact data: {value}]").ok();
                }
            }
        }
    }

    fn artifact_filename(artifact: &a2a::Artifact, part: &a2a::Part) -> String {
        part.filename
            .clone()
            .or_else(|| artifact.name.clone())
            .unwrap_or_else(|| "artifact".to_string())
    }

    async fn save_bytes_and_note(
        &self,
        body: &mut String,
        artifact: &a2a::Artifact,
        part: &a2a::Part,
        bytes: Vec<u8>,
    ) {
        let filename = Self::artifact_filename(artifact, part);
        let info = AttachmentInfo {
            filename: filename.clone(),
            size: u32::try_from(bytes.len()).unwrap_or(u32::MAX),
            content_type: part.media_type.clone(),
        };
        match attachment::save_attachment_bytes(&info, &bytes, &self.inbox_dir).await {
            Ok(saved) => {
                write!(
                    body,
                    "\n\n{}",
                    attachment::format_attachment_line(&saved, &info)
                )
                .ok();
            }
            Err(e) => {
                write!(body, "\n\n[Artifact: {filename} — failed to save: {e}]").ok();
            }
        }
    }

    async fn save_url_and_note(
        &self,
        body: &mut String,
        artifact: &a2a::Artifact,
        part: &a2a::Part,
        url: &str,
    ) {
        let filename = Self::artifact_filename(artifact, part);
        let info = AttachmentInfo {
            filename: filename.clone(),
            size: 0,
            content_type: part.media_type.clone(),
        };
        match attachment::download_attachment(&info, url, &self.inbox_dir).await {
            Ok(saved) => {
                write!(
                    body,
                    "\n\n{}",
                    attachment::format_attachment_line(&saved, &info)
                )
                .ok();
            }
            Err(e) => {
                write!(body, "\n\n[Artifact: {filename} — failed to download: {e}]").ok();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a2a::client::hub::AgentSource;
    use crate::background::HopLimits;
    use crate::background::registry::SessionRegistry;
    use crate::background::store::SessionStore;

    fn messenger() -> (Arc<AgentMessenger>, crate::bus::BusHandle) {
        let bus_handle = crate::bus::spawn_broker();
        let registry = Arc::new(SessionRegistry::new());
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(SessionStore::new(dir.keep()));
        let messenger = Arc::new(AgentMessenger::new(
            registry,
            bus_handle.publisher(),
            store,
            HopLimits { soft: 8, hard: 32 },
        ));
        (messenger, bus_handle)
    }

    #[tokio::test]
    async fn track_persists_and_resumes_on_reload() {
        let dir = tempfile::tempdir().unwrap();
        let outbound_path = dir.path().join("outbound.json");
        let hub = A2aClientHub::new_shared();
        let (messenger, _bus) = messenger();
        let inbox_dir = dir.path().join("inbox");
        tokio::fs::create_dir_all(&inbox_dir).await.unwrap();

        let tracker = RemoteTaskTracker::load(
            outbound_path.clone(),
            Arc::clone(&hub),
            Arc::clone(&messenger),
            inbox_dir.clone(),
        )
        .await;
        tracker
            .track(
                &SessionAddress::from("main"),
                "laptop",
                "task-1".to_string(),
                "ctx-1".to_string(),
                "working",
                0,
            )
            .await;

        assert!(outbound_path.exists());
        assert_eq!(
            tracker.context_for("main", "laptop").await,
            Some("ctx-1".to_string())
        );

        // Rebuild the tracker from the persisted file, as a restart would.
        let rebuilt = RemoteTaskTracker::load(outbound_path, hub, messenger, inbox_dir).await;
        assert_eq!(
            rebuilt.context_for("main", "laptop").await,
            Some("ctx-1".to_string())
        );
        let open = rebuilt.any_open_task_for("main", "laptop").await.unwrap();
        assert_eq!(open.task_id, "task-1");
    }

    #[tokio::test]
    async fn awaiting_reply_task_only_matches_input_or_auth_required() {
        let dir = tempfile::tempdir().unwrap();
        let hub = A2aClientHub::new_shared();
        let (messenger, _bus) = messenger();
        let tracker = RemoteTaskTracker::load(
            dir.path().join("outbound.json"),
            hub,
            messenger,
            dir.path().join("inbox"),
        )
        .await;
        tracker
            .track(
                &SessionAddress::from("main"),
                "laptop",
                "t1".to_string(),
                "c1".to_string(),
                "working",
                0,
            )
            .await;
        assert!(
            tracker
                .awaiting_reply_task_for("main", "laptop")
                .await
                .is_none()
        );

        tracker
            .track(
                &SessionAddress::from("main"),
                "laptop",
                "t1".to_string(),
                "c1".to_string(),
                "input_required",
                0,
            )
            .await;
        assert!(
            tracker
                .awaiting_reply_task_for("main", "laptop")
                .await
                .is_some()
        );
    }

    #[tokio::test]
    async fn note_unreachable_starts_a_streak_once_and_recovery_clears_it() {
        let dir = tempfile::tempdir().unwrap();
        let hub = A2aClientHub::new_shared();
        let (messenger, _bus) = messenger();
        let tracker = RemoteTaskTracker::load(
            dir.path().join("outbound.json"),
            hub,
            messenger,
            dir.path().join("inbox"),
        )
        .await;
        tracker
            .track(
                &SessionAddress::from("main"),
                "laptop",
                "t1".to_string(),
                "c1".to_string(),
                "working",
                0,
            )
            .await;

        tracker.note_unreachable("t1", "connection refused").await;
        let after_first_failure = tracker.get("t1").await.unwrap();
        let first_seen = after_first_failure.first_unreachable_at;
        assert!(
            first_seen.is_some(),
            "a failure should start the unreachable streak"
        );

        // A second failure doesn't move the streak's start time — this is
        // what keeps note_unreachable's own warn log to once per streak
        // instead of once per poll.
        tracker.note_unreachable("t1", "connection refused").await;
        let after_second_failure = tracker.get("t1").await.unwrap();
        assert_eq!(
            after_second_failure.first_unreachable_at, first_seen,
            "the streak start should not move on repeated failures"
        );

        // A successful poll clears the streak, which is what the matching
        // recovery log in update_state fires on.
        tracker
            .update_state("t1", "working", None, false)
            .await
            .unwrap();
        let after_recovery = tracker.get("t1").await.unwrap();
        assert!(
            after_recovery.first_unreachable_at.is_none(),
            "recovery should clear the unreachable streak"
        );
    }

    #[tokio::test]
    async fn pruning_drops_old_terminal_tasks_on_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("outbound.json");
        let mut store = OutboundStore::default();
        let old = Utc::now() - chrono::Duration::days(31);
        store.tasks.insert(
            "old-task".to_string(),
            TrackedTask {
                sender_address: "main".to_string(),
                agent: "laptop".to_string(),
                task_id: "old-task".to_string(),
                context_id: "c1".to_string(),
                state: "completed".to_string(),
                last_status_text: None,
                hop_count: 0,
                created_at: old,
                updated_at: old,
                first_unreachable_at: None,
                unreachable_notified: false,
                notified_this_turn: false,
                stopped_by_user: false,
            },
        );
        tokio::fs::write(&path, serde_json::to_vec(&store).unwrap())
            .await
            .unwrap();

        let hub = A2aClientHub::new_shared();
        let (messenger, _bus) = messenger();
        let tracker = RemoteTaskTracker::load(path, hub, messenger, dir.path().join("inbox")).await;
        assert!(tracker.get("old-task").await.is_none());
    }

    #[tokio::test]
    async fn cancel_open_task_reports_none_when_nothing_is_open() {
        let dir = tempfile::tempdir().unwrap();
        let hub = A2aClientHub::new_shared();
        let (messenger, _bus) = messenger();
        let tracker = RemoteTaskTracker::load(
            dir.path().join("outbound.json"),
            hub,
            messenger,
            dir.path().join("inbox"),
        )
        .await;
        let result = tracker.cancel_open_task("main", "laptop").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn cancel_open_task_reports_hub_error_when_agent_is_unreachable() {
        let dir = tempfile::tempdir().unwrap();
        let hub = A2aClientHub::new_shared();
        hub.register_external(
            "laptop".to_string(),
            "http://127.0.0.1:1".to_string(),
            HashMap::new(),
            AgentSource::Config,
        )
        .await;
        let (messenger, _bus) = messenger();
        let tracker = RemoteTaskTracker::load(
            dir.path().join("outbound.json"),
            hub,
            messenger,
            dir.path().join("inbox"),
        )
        .await;
        tracker
            .track(
                &SessionAddress::from("main"),
                "laptop",
                "t1".to_string(),
                "c1".to_string(),
                "working",
                0,
            )
            .await;
        let err = tracker
            .cancel_open_task("main", "laptop")
            .await
            .unwrap_err();
        assert!(matches!(err, HubError::Offline(name, _) if name == "laptop"));
    }

    #[tokio::test]
    async fn note_unreachable_notifies_once_after_ten_minutes_not_on_every_poll() {
        let dir = tempfile::tempdir().unwrap();
        let hub = A2aClientHub::new_shared();
        let (messenger, bus) = messenger();
        let mut notices = bus
            .subscribe::<_, NoticeEvent>(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
            .await
            .unwrap();
        let tracker = RemoteTaskTracker::load(
            dir.path().join("outbound.json"),
            hub,
            messenger,
            dir.path().join("inbox"),
        )
        .await;
        tracker
            .track(
                &SessionAddress::from("main"),
                "laptop",
                "t1".to_string(),
                "c1".to_string(),
                "working",
                0,
            )
            .await;

        // First failure starts the streak; under the 10-minute threshold,
        // so no user notice yet.
        tracker.note_unreachable("t1", "connection refused").await;
        assert!(
            tokio::time::timeout(Duration::from_millis(50), notices.recv())
                .await
                .is_err(),
            "must not notify before the streak is 10 minutes old"
        );

        // Backdate the streak's start past the threshold and fail again.
        {
            let mut store = tracker.store.write().await;
            let entry = store.tasks.get_mut("t1").unwrap();
            entry.first_unreachable_at = Some(Utc::now() - chrono::Duration::minutes(11));
        }
        tracker.note_unreachable("t1", "connection refused").await;
        let notice = notices.recv().await.unwrap().unwrap();
        assert!(notice.message.contains("laptop"), "got: {}", notice.message);
        assert!(
            notice.message.contains("10 minutes"),
            "got: {}",
            notice.message
        );

        // A third failure in the same streak must not renotify.
        tracker.note_unreachable("t1", "connection refused").await;
        assert!(
            tokio::time::timeout(Duration::from_millis(50), notices.recv())
                .await
                .is_err(),
            "must not renotify within the same unreachable streak"
        );
    }

    #[tokio::test]
    async fn update_state_notifies_recovery_once_after_an_unreachable_notice() {
        let dir = tempfile::tempdir().unwrap();
        let hub = A2aClientHub::new_shared();
        let (messenger, bus) = messenger();
        let mut notices = bus
            .subscribe::<_, NoticeEvent>(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
            .await
            .unwrap();
        let tracker = RemoteTaskTracker::load(
            dir.path().join("outbound.json"),
            hub,
            messenger,
            dir.path().join("inbox"),
        )
        .await;
        tracker
            .track(
                &SessionAddress::from("main"),
                "laptop",
                "t1".to_string(),
                "c1".to_string(),
                "working",
                0,
            )
            .await;

        // Simulate an already-notified unreachable streak directly, rather
        // than waiting out the real threshold.
        {
            let mut store = tracker.store.write().await;
            let entry = store.tasks.get_mut("t1").unwrap();
            entry.first_unreachable_at = Some(Utc::now() - chrono::Duration::minutes(11));
            entry.unreachable_notified = true;
        }

        tracker.update_state("t1", "working", None, false).await;
        let notice = notices.recv().await.unwrap().unwrap();
        assert!(
            notice.message.contains("reachable again"),
            "got: {}",
            notice.message
        );

        // A second successful contact must not renotify — recovery is
        // reported exactly once per streak, like the unreachable notice
        // itself.
        tracker.update_state("t1", "working", None, false).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(50), notices.recv())
                .await
                .is_err(),
            "must not renotify recovery on a later successful contact"
        );
    }

    #[tokio::test]
    async fn update_state_does_not_notify_when_never_unreachable() {
        let dir = tempfile::tempdir().unwrap();
        let hub = A2aClientHub::new_shared();
        let (messenger, bus) = messenger();
        let mut notices = bus
            .subscribe::<_, NoticeEvent>(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
            .await
            .unwrap();
        let tracker = RemoteTaskTracker::load(
            dir.path().join("outbound.json"),
            hub,
            messenger,
            dir.path().join("inbox"),
        )
        .await;
        tracker
            .track(
                &SessionAddress::from("main"),
                "laptop",
                "t1".to_string(),
                "c1".to_string(),
                "working",
                0,
            )
            .await;

        tracker.update_state("t1", "working", None, false).await;

        assert!(
            tokio::time::timeout(Duration::from_millis(50), notices.recv())
                .await
                .is_err(),
            "a task that was never unreachable has no recovery to report"
        );
    }

    async fn tracker_with_task(
        dir: &std::path::Path,
        hub: Arc<A2aClientHub>,
        messenger: Arc<AgentMessenger>,
    ) -> Arc<RemoteTaskTracker> {
        let tracker =
            RemoteTaskTracker::load(dir.join("outbound.json"), hub, messenger, dir.join("inbox"))
                .await;
        tracker
            .track(
                &SessionAddress::from("main"),
                "laptop",
                "t1".to_string(),
                "c1".to_string(),
                "working",
                0,
            )
            .await;
        tracker
    }

    #[tokio::test]
    async fn open_tasks_lists_only_open_tasks_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let (messenger, _bus) = messenger();
        let tracker = tracker_with_task(dir.path(), A2aClientHub::new_shared(), messenger).await;
        tracker
            .track(
                &SessionAddress::from("main"),
                "desktop",
                "t2".to_string(),
                "c2".to_string(),
                "working",
                0,
            )
            .await;
        tracker
            .track(
                &SessionAddress::from("main"),
                "phone",
                "t3".to_string(),
                "c3".to_string(),
                "working",
                0,
            )
            .await;
        {
            let mut store = tracker.store.write().await;
            let t2 = store.tasks.get_mut("t2").unwrap();
            t2.created_at = Utc::now() + chrono::Duration::seconds(5);
            store.tasks.get_mut("t3").unwrap().state = "completed".to_string();
        }

        let ids: Vec<String> = tracker
            .open_tasks()
            .await
            .into_iter()
            .map(|t| t.task_id)
            .collect();
        assert_eq!(ids, ["t2", "t1"]);
    }

    #[tokio::test]
    async fn track_and_state_changes_publish_a_task_event() {
        let dir = tempfile::tempdir().unwrap();
        let (messenger, bus) = messenger();
        let mut events = bus
            .subscribe::<_, OutboundA2aTaskEvent>(topics::Notification(NotifyName::from(
                SYSTEM_CHANNEL,
            )))
            .await
            .unwrap();
        let tracker = tracker_with_task(dir.path(), A2aClientHub::new_shared(), messenger).await;

        let recorded = events.recv().await.unwrap().unwrap();
        assert_eq!(recorded.task.task_id, "t1");
        assert_eq!(recorded.task.state, "working");

        // A poll that changes nothing the sidebar shows publishes nothing.
        tracker.update_state("t1", "working", None, false).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(50), events.recv())
                .await
                .is_err(),
            "an unchanged poll must not publish"
        );

        tracker
            .update_state("t1", "completed", Some("done".to_string()), true)
            .await;
        let finished = events.recv().await.unwrap().unwrap();
        assert_eq!(finished.task.state, "completed");
        assert!(!finished.task.is_open());
    }

    #[tokio::test]
    async fn stop_task_reports_none_for_an_unknown_task() {
        let dir = tempfile::tempdir().unwrap();
        let (messenger, _bus) = messenger();
        let tracker = tracker_with_task(dir.path(), A2aClientHub::new_shared(), messenger).await;
        assert!(tracker.stop_task("nope").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn stop_task_reports_hub_error_when_agent_is_unreachable() {
        let dir = tempfile::tempdir().unwrap();
        let hub = A2aClientHub::new_shared();
        hub.register_external(
            "laptop".to_string(),
            "http://127.0.0.1:1".to_string(),
            HashMap::new(),
            AgentSource::Config,
        )
        .await;
        let (messenger, _bus) = messenger();
        let tracker = tracker_with_task(dir.path(), hub, messenger).await;

        let err = tracker.stop_task("t1").await.unwrap_err();
        assert!(matches!(err, HubError::Offline(name, _) if name == "laptop"));
        assert!(
            tracker.get("t1").await.unwrap().is_open(),
            "a failed stop leaves the task open for stop_watching"
        );
    }

    #[tokio::test]
    async fn stop_watching_closes_the_task_tells_the_sender_and_ignores_late_updates() {
        let dir = tempfile::tempdir().unwrap();
        let (messenger, bus) = messenger();
        let mut to_main = bus
            .subscribe::<_, crate::bus::MessageEvent>(topics::UserMessage)
            .await
            .unwrap();
        let tracker = tracker_with_task(dir.path(), A2aClientHub::new_shared(), messenger).await;

        let stopped = tracker.stop_watching("t1").await.unwrap();
        assert_eq!(stopped.state, "canceled");
        assert!(tracker.open_tasks().await.is_empty());

        let told = to_main.recv().await.unwrap().unwrap();
        assert!(
            told.content.contains("stopped watching"),
            "got: {}",
            told.content
        );

        assert!(
            tracker
                .update_state("t1", "working", None, false)
                .await
                .is_none(),
            "a late update must not reopen a task the user closed"
        );
        assert_eq!(tracker.get("t1").await.unwrap().state, "canceled");
        assert!(
            tracker.stop_watching("t1").await.is_none(),
            "an already-closed task has nothing to stop"
        );
    }
}
