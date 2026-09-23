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
use crate::bus::SessionAddress;
use crate::interfaces::attachment::{self, AttachmentInfo};

use super::hub::{A2aClientHub, HubError, task_state_str};

/// How long an agent must stay unreachable before the sender gets a single
/// notice; retries continue either way.
const UNREACHABLE_NOTICE_AFTER: chrono::Duration = chrono::Duration::hours(3);
/// How long a completed task's record is kept before being pruned on load.
const PRUNE_TERMINAL_AFTER: chrono::Duration = chrono::Duration::days(30);
/// Poll/reconnect backoff bounds, matching the plan's 5s→60s.
const MIN_BACKOFF: Duration = Duration::from_secs(5);
const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// Text parts at or under this size are delivered inline; larger ones (and
/// every file part) are saved to the agent inbox instead.
const INLINE_TEXT_LIMIT: usize = 4096;

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
    /// Whether the one-time 3-hour unreachable notice has already fired for
    /// the current unreachable streak.
    #[serde(default)]
    pub unreachable_notified: bool,
    /// Whether a delivery has already fired for the current turn (since the
    /// last [`RemoteTaskTracker::track`] call) — a direct cancel and the
    /// background watcher can both observe the same terminal event for one
    /// turn, and only the first should reach the sender. Reset to `false`
    /// every time `track` records a new send or follow-up.
    #[serde(default)]
    pub notified_this_turn: bool,
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
                });
            entry.state = state.to_string();
            entry.hop_count = hop_count;
            entry.context_id = context_id;
            entry.updated_at = now;
            entry.first_unreachable_at = None;
            entry.unreachable_notified = false;
            entry.notified_this_turn = false;
            store.clone()
        };
        self.persist(&snapshot).await;
        self.spawn_watch(task_id);
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
        let (client, _card) = self.hub.client_for(agent).await?;
        match client
            .cancel_task(&a2a::CancelTaskRequest {
                id: task.task_id.clone(),
                metadata: None,
                tenant: None,
            })
            .await
        {
            Ok(remote_task) => {
                self.apply_remote_task(remote_task).await;
            }
            Err(e) => {
                tracing::warn!(task_id = %task.task_id, agent, error = %e, "failed to cancel a2a remote task");
                return Err(HubError::ClientBuild(agent.to_string(), e.to_string()));
            }
        }
        Ok(Some(task.task_id))
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
        let (result, snapshot) = {
            let mut store = self.store.write().await;
            let entry = store.tasks.get_mut(task_id)?;
            entry.state = state.to_string();
            if let Some(t) = text {
                entry.last_status_text = Some(t);
            }
            entry.updated_at = Utc::now();
            entry.first_unreachable_at = None;
            entry.unreachable_notified = false;
            let should_deliver = is_final && !entry.notified_this_turn;
            if should_deliver {
                entry.notified_this_turn = true;
            }
            ((entry.clone(), should_deliver), store.clone())
        };
        self.persist(&snapshot).await;
        Some(result)
    }

    /// Record an unreachable attempt against `task_id`. After the streak
    /// passes [`UNREACHABLE_NOTICE_AFTER`], sends the sender exactly one
    /// notice — retries continue regardless.
    async fn note_unreachable(&self, task_id: &str, reason: &str) {
        let outcome = {
            let mut store = self.store.write().await;
            let Some(entry) = store.tasks.get_mut(task_id) else {
                return;
            };
            let now = Utc::now();
            let first = *entry.first_unreachable_at.get_or_insert(now);
            let should_notify =
                !entry.unreachable_notified && now - first >= UNREACHABLE_NOTICE_AFTER;
            if should_notify {
                entry.unreachable_notified = true;
            }
            (
                should_notify,
                entry.sender_address.clone(),
                entry.agent.clone(),
                entry.hop_count,
                store.clone(),
            )
        };
        let (should_notify, sender, agent, hop_count, snapshot) = outcome;
        self.persist(&snapshot).await;
        tracing::warn!(task_id, agent = %agent, reason, "a2a remote task unreachable");
        if should_notify {
            let content = format!(
                "[Remote agent a2a:{agent} — task {task_id}] Still unreachable after 3 hours \
                 ({reason}). Still retrying in the background."
            );
            self.deliver_to_sender(&sender, &agent, hop_count, content)
                .await;
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
            let mut stream = match client.subscribe_to_task(&req).await {
                Ok(s) => s,
                Err(e) => {
                    self.note_unreachable(task_id, &e.to_string()).await;
                    tokio::time::sleep(MAX_BACKOFF).await;
                    continue;
                }
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
            tokio::time::sleep(MIN_BACKOFF).await;
        }
    }

    async fn deliver(&self, task: &TrackedTask, text: Option<&str>, artifacts: Vec<a2a::Artifact>) {
        let mut body = format!(
            "[Remote agent a2a:{agent} — task {task_id}: {state}]\n{text}",
            agent = task.agent,
            task_id = task.task_id,
            state = task.state,
            text = text.unwrap_or("(no status message)"),
        );
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
}
