use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use futures_util::StreamExt as _;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use residuum_background::messaging::{AgentMessenger, DeliveryOutcome};
use residuum_bus::remote_task::{TrackedTask, UNREACHABLE_NOTICE_AFTER};
use residuum_bus::session::MAIN_ADDRESS;
use residuum_bus::{
    NoticeEvent, NotifyName, OutboundA2aTaskEvent, SYSTEM_CHANNEL, SessionAddress, topics,
};
use residuum_workspace::inbox::attachment::{self, AttachmentInfo};

use super::hub::{A2aClientHub, HubError, task_state_str};

const PRUNE_TERMINAL_AFTER: chrono::Duration = chrono::Duration::days(30);
const MIN_BACKOFF: Duration = Duration::from_secs(5);
const MAX_BACKOFF: Duration = Duration::from_secs(60);
const INLINE_TEXT_LIMIT: usize = 4096;
const USER_STOPPED_NOTE: &str = "Stopped by the user from the web UI.";

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct OutboundStore {
    #[serde(default)]
    tasks: HashMap<String, TrackedTask>,
    #[serde(default)]
    contexts: HashMap<String, HashMap<String, String>>,
}

pub async fn read_open_tasks(path: &std::path::Path) -> anyhow::Result<Vec<TrackedTask>> {
    use anyhow::Context as _;

    let raw = match tokio::fs::read_to_string(path).await {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(e).with_context(|| {
                format!("failed to read a2a outbound tasks at {}", path.display())
            });
        }
    };
    let store: OutboundStore = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse a2a outbound tasks at {}", path.display()))?;
    let mut tasks: Vec<TrackedTask> = store
        .tasks
        .into_values()
        .filter(TrackedTask::is_open)
        .collect();
    tasks.sort_by_key(|task| std::cmp::Reverse(task.created_at));
    Ok(tasks)
}

pub struct RemoteTaskTracker {
    path: PathBuf,
    store: RwLock<OutboundStore>,
    hub: Arc<A2aClientHub>,
    messenger: Arc<AgentMessenger>,
    inbox_dir: PathBuf,
    timezone: chrono_tz::Tz,
    shutdown: tokio_util::sync::CancellationToken,
}

impl RemoteTaskTracker {
    #[must_use]
    pub async fn load(
        path: PathBuf,
        hub: Arc<A2aClientHub>,
        messenger: Arc<AgentMessenger>,
        inbox_dir: PathBuf,
        timezone: chrono_tz::Tz,
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
            timezone,
            shutdown: tokio_util::sync::CancellationToken::new(),
        })
    }

    pub fn shutdown(&self) {
        self.shutdown.cancel();
    }

    async fn persist(&self, snapshot: &OutboundStore) {
        match serde_json::to_vec_pretty(snapshot) {
            Ok(bytes) => {
                if let Err(e) = residuum_core::util::fs::atomic_write(&self.path, bytes).await {
                    tracing::error!(error = %e, path = %self.path.display(), "failed to persist a2a outbound tasks");
                }
            }
            Err(e) => tracing::error!(error = %e, "failed to serialize a2a outbound tasks"),
        }
    }

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
        residuum_core::util::spawn_monitored("a2a-task-watch", async move {
            let shutdown = tracker.shutdown.clone();
            tokio::select! {
                () = shutdown.cancelled() => {}
                () = tracker.watch_loop(task_id) => {}
            }
        });
    }

    pub async fn awaiting_reply_task_for(&self, sender: &str, agent: &str) -> Option<TrackedTask> {
        let store = self.store.read().await;
        store
            .tasks
            .values()
            .find(|t| t.sender_address == sender && t.agent == agent && t.awaits_reply())
            .cloned()
    }

    pub async fn any_open_task_for(&self, sender: &str, agent: &str) -> Option<TrackedTask> {
        let store = self.store.read().await;
        store
            .tasks
            .values()
            .find(|t| t.sender_address == sender && t.agent == agent && t.is_open())
            .cloned()
    }

    pub async fn context_for(&self, sender: &str, agent: &str) -> Option<String> {
        self.store
            .read()
            .await
            .contexts
            .get(sender)
            .and_then(|m| m.get(agent))
            .cloned()
    }

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

    pub async fn apply_sent_task(&self, task: a2a::Task) {
        self.apply_remote_task(task).await;
    }

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

    pub async fn stop_task(&self, task_id: &str) -> Result<Option<TrackedTask>, HubError> {
        let Some(task) = self.get(task_id).await.filter(TrackedTask::is_open) else {
            return Ok(None);
        };
        self.cancel_remote(&task, Some(USER_STOPPED_NOTE)).await?;
        Ok(self.get(task_id).await)
    }

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
        if (is_streak_start || should_notify)
            && let Some(task) = snapshot.tasks.get(task_id)
        {
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

    async fn publish_user_notice(&self, message: &str) {
        if let Err(e) = self
            .messenger
            .publisher()
            .publish(
                topics::Notification(NotifyName::from(SYSTEM_CHANNEL)),
                NoticeEvent::new(message.to_string()),
            )
            .await
        {
            tracing::warn!(error = %e, "failed to publish a2a task notice");
        }
    }

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
                residuum_bus::AgentMessageEvent::new(
                    from.clone(),
                    "remote",
                    content.clone(),
                    hop_count,
                ),
            )
            .await
        {
            Ok(DeliveryOutcome::Unknown) => {
                let note = format!(
                    "[Remote agent a2a:{agent} sent a reply for {sender}, which no longer exists]\n{content}"
                );
                if let Err(e) = self
                    .messenger
                    .send(
                        MAIN_ADDRESS,
                        residuum_bus::AgentMessageEvent::new(from, "remote", note, hop_count),
                    )
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

    async fn apply_remote_task(&self, task: a2a::Task) -> bool {
        self.apply_remote_task_noted(task, None).await
    }

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
            if self.poll_once(&client, task_id).await {
                return;
            }

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
                    Ok(a2a::StreamResponse::Message(_)) => {}
                    Err(e) => {
                        self.note_unreachable(task_id, &e.to_string()).await;
                        break;
                    }
                }
            }
            if delivered {
                return;
            }
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
            self.append_artifact(&mut body, &task.agent, &artifact)
                .await;
        }
        self.deliver_to_sender(&task.sender_address, &task.agent, task.hop_count, body)
            .await;
    }

    async fn append_artifact(&self, body: &mut String, agent: &str, artifact: &a2a::Artifact) {
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
                    self.save_bytes_and_note(
                        body,
                        agent,
                        artifact,
                        part,
                        text.clone().into_bytes(),
                    )
                    .await;
                }
                a2a::PartContent::Raw(bytes) => {
                    self.save_bytes_and_note(body, agent, artifact, part, bytes.clone())
                        .await;
                }
                a2a::PartContent::Url(url) => {
                    self.save_url_and_note(body, agent, artifact, part, url)
                        .await;
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
        agent: &str,
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
        let item = match self.reserve_artifact(&info).await {
            Ok(item) => item,
            Err(e) => {
                Self::note_failed_artifact(body, agent, &filename, "save", &e);
                return;
            }
        };
        let saved = item.save_bytes(&info, &bytes).await;
        match saved {
            Ok(saved) => {
                self.note_saved_artifact(body, agent, &saved, &info, item)
                    .await;
            }
            Err(e) => {
                item.discard().await;
                Self::note_failed_artifact(body, agent, &filename, "save", &e);
            }
        }
    }

    async fn save_url_and_note(
        &self,
        body: &mut String,
        agent: &str,
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
        let item = match self.reserve_artifact(&info).await {
            Ok(item) => item,
            Err(e) => {
                Self::note_failed_artifact(body, agent, &filename, "download", &e);
                return;
            }
        };
        let downloaded = item.download(&info, url).await;
        match downloaded {
            Ok(saved) => {
                self.note_saved_artifact(body, agent, &saved, &info, item)
                    .await;
            }
            Err(e) => {
                item.discard().await;
                Self::note_failed_artifact(body, agent, &filename, "download", &e);
            }
        }
    }

    fn note_failed_artifact(
        body: &mut String,
        agent: &str,
        filename: &str,
        action: &str,
        reason: &str,
    ) {
        tracing::warn!(
            agent = %agent,
            filename = %filename,
            action,
            error = %reason,
            "failed to store a2a artifact"
        );
        write!(
            body,
            "\n\n[Artifact: {filename} — failed to {action}: {reason}]"
        )
        .ok();
    }

    async fn reserve_artifact(
        &self,
        info: &AttachmentInfo,
    ) -> Result<attachment::AttachmentItem, String> {
        attachment::reserve_attachment_item(
            &self.inbox_dir,
            info,
            "A2A",
            attachment::AttachmentKind::Artifact,
            self.timezone,
        )
        .await
    }

    async fn note_saved_artifact(
        &self,
        body: &mut String,
        agent: &str,
        saved: &attachment::SavedAttachment,
        info: &AttachmentInfo,
        item: attachment::AttachmentItem,
    ) {
        write!(
            body,
            "\n\n{}",
            attachment::format_attachment_line(saved, info)
        )
        .ok();
        item.file_companion_item(saved, info, agent).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::hub::AgentSource;
    use residuum_agent::hop::HopLimits;
    use residuum_background::registry::SessionRegistry;
    use residuum_background::store::SessionStore;
    use residuum_bus::testing::bus_barrier;
    use residuum_core::testing::wait;
    use std::path::Path;

    fn messenger() -> (Arc<AgentMessenger>, residuum_bus::BusHandle) {
        let bus_handle = residuum_bus::spawn_broker();
        let registry = Arc::new(SessionRegistry::new());
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(SessionStore::new(dir.path().to_path_buf()).with_tempdir_guard(dir));
        let messenger = Arc::new(AgentMessenger::new(
            registry,
            bus_handle.publisher(),
            store,
            HopLimits { soft: 8, hard: 32 },
        ));
        (messenger, bus_handle)
    }

    #[tokio::test]
    async fn raw_artifact_is_filed_under_its_own_agent_inbox_item() {
        let dir = tempfile::tempdir().unwrap();
        let inbox_dir = dir.path().join("inbox");
        tokio::fs::create_dir_all(&inbox_dir).await.unwrap();
        let (messenger, _bus) = messenger();
        let tracker = RemoteTaskTracker::load(
            dir.path().join("outbound.json"),
            A2aClientHub::new_shared(),
            messenger,
            inbox_dir.clone(),
            chrono_tz::UTC,
        )
        .await;
        let artifact = a2a::Artifact {
            artifact_id: "artifact-1".to_string(),
            name: Some("report".to_string()),
            description: None,
            parts: vec![a2a::Part::raw(b"bytes".to_vec()).with_filename("report.bin")],
            metadata: None,
            extensions: None,
        };

        let mut body = String::from("body");
        tracker
            .append_artifact(&mut body, "remote-agent", &artifact)
            .await;

        let mut item_paths = Vec::new();
        let mut entries = tokio::fs::read_dir(&inbox_dir).await.unwrap();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "json") {
                item_paths.push(path);
            }
        }
        let [item_path] = item_paths.as_slice() else {
            panic!("one companion item per artifact file: {item_paths:?}");
        };
        let id = item_path
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let item = residuum_workspace::inbox::load_item(item_path)
            .await
            .unwrap();
        assert_eq!(item.title, "A2A artifact: report.bin");
        assert_eq!(item.source, "a2a");
        let body_text = &item.body;
        assert!(body_text.contains("From: remote-agent"), "{body_text}");
        assert!(body_text.contains("Size: 5 bytes"), "{body_text}");
        let [recorded] = item.attachments.as_slice() else {
            panic!("one attachment recorded: {:?}", item.attachments);
        };
        assert!(
            recorded.ends_with(Path::new("attachments").join(&id).join("report.bin")),
            "item should record its file under its own directory: {}",
            recorded.display()
        );

        let saved = inbox_dir.join("attachments").join(&id).join("report.bin");
        assert_eq!(tokio::fs::read(&saved).await.unwrap(), b"bytes");
        assert!(
            body.contains(&saved.display().to_string()),
            "body should point at the saved path: {body}"
        );
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
            chrono_tz::UTC,
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

        let rebuilt =
            RemoteTaskTracker::load(outbound_path, hub, messenger, inbox_dir, chrono_tz::UTC).await;
        assert_eq!(
            rebuilt.context_for("main", "laptop").await,
            Some("ctx-1".to_string())
        );
        let open = rebuilt.any_open_task_for("main", "laptop").await.unwrap();
        assert_eq!(open.task_id, "task-1");
    }

    #[tokio::test]
    async fn shutdown_ends_the_watchers_of_open_tasks() {
        let dir = tempfile::tempdir().unwrap();
        let hub = A2aClientHub::new_shared();
        let (messenger, _bus) = messenger();
        let tracker = RemoteTaskTracker::load(
            dir.path().join("outbound.json"),
            hub,
            messenger,
            dir.path().join("inbox"),
            chrono_tz::UTC,
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
        tracker.spawn_resume_watchers().await;
        assert!(
            Arc::strong_count(&tracker) > 1,
            "the open task has a watcher holding the tracker"
        );

        tracker.shutdown();
        wait::until_true("the watchers to drop the tracker after shutdown", || {
            Arc::strong_count(&tracker) == 1
        })
        .await;
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
            chrono_tz::UTC,
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
            chrono_tz::UTC,
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

        tracker.note_unreachable("t1", "connection refused").await;
        let after_second_failure = tracker.get("t1").await.unwrap();
        assert_eq!(
            after_second_failure.first_unreachable_at, first_seen,
            "the streak start should not move on repeated failures"
        );

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
        let tracker = RemoteTaskTracker::load(
            path,
            hub,
            messenger,
            dir.path().join("inbox"),
            chrono_tz::UTC,
        )
        .await;
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
            chrono_tz::UTC,
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
            chrono_tz::UTC,
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
            chrono_tz::UTC,
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
        bus_barrier(&bus).await;
        assert!(
            notices.drain().is_empty(),
            "must not notify before the streak is 10 minutes old"
        );

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

        tracker.note_unreachable("t1", "connection refused").await;
        bus_barrier(&bus).await;
        assert!(
            notices.drain().is_empty(),
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
            chrono_tz::UTC,
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

        tracker.update_state("t1", "working", None, false).await;
        bus_barrier(&bus).await;
        assert!(
            notices.drain().is_empty(),
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
            chrono_tz::UTC,
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

        bus_barrier(&bus).await;
        assert!(
            notices.drain().is_empty(),
            "a task that was never unreachable has no recovery to report"
        );
    }

    async fn tracker_with_task(
        dir: &std::path::Path,
        hub: Arc<A2aClientHub>,
        messenger: Arc<AgentMessenger>,
    ) -> Arc<RemoteTaskTracker> {
        let tracker = RemoteTaskTracker::load(
            dir.join("outbound.json"),
            hub,
            messenger,
            dir.join("inbox"),
            chrono_tz::UTC,
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
        tracker
    }

    #[tokio::test]
    async fn a_reader_outside_the_tracker_sees_the_open_tasks_it_persisted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("outbound.json");
        assert!(
            read_open_tasks(&path).await.unwrap().is_empty(),
            "a missing file has no tasks"
        );

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
            .update_state("t2", "completed", Some("done".to_string()), true)
            .await;

        let tasks = read_open_tasks(&path).await.unwrap();
        assert_eq!(
            tasks.iter().map(|t| t.task_id.as_str()).collect::<Vec<_>>(),
            ["t1"],
            "only the open task, and nothing was pruned or rewritten"
        );

        std::fs::write(&path, "{ not json").unwrap();
        let err = read_open_tasks(&path).await.unwrap_err();
        assert!(format!("{err:#}").contains("outbound.json"), "{err:#}");
    }

    #[tokio::test]
    async fn a_task_in_an_unreachable_streak_knows_when_the_streak_passes_the_notice_threshold() {
        let dir = tempfile::tempdir().unwrap();
        let (messenger, _bus) = messenger();
        let tracker = tracker_with_task(dir.path(), A2aClientHub::new_shared(), messenger).await;
        let task = tracker.get("t1").await.unwrap();
        assert_eq!(task.notice_due_at(), None, "reachable");

        tracker.note_unreachable("t1", "connection refused").await;
        let in_streak = tracker.get("t1").await.unwrap();
        assert_eq!(
            in_streak.notice_due_at(),
            Some(in_streak.first_unreachable_at.unwrap() + UNREACHABLE_NOTICE_AFTER)
        );

        let mut stopped = in_streak.clone();
        stopped.stopped_by_user = true;
        assert_eq!(
            stopped.notice_due_at(),
            None,
            "the user stopped watching it"
        );
        let mut closed = in_streak;
        closed.state = "completed".to_string();
        assert_eq!(
            closed.notice_due_at(),
            None,
            "a closed task isn't a problem"
        );
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

        tracker.update_state("t1", "working", None, false).await;
        bus_barrier(&bus).await;
        assert!(
            events.drain().is_empty(),
            "an unchanged poll must not publish"
        );

        tracker
            .update_state("t1", "completed", Some("done".to_string()), true)
            .await;
        let finished = events.recv().await.unwrap().unwrap();
        assert_eq!(finished.task.state, "completed");
        assert!(!finished.task.is_open());
    }

    async fn task_events_now(
        bus: &residuum_bus::BusHandle,
        events: &mut residuum_bus::Subscriber<OutboundA2aTaskEvent>,
    ) -> Vec<TrackedTask> {
        bus_barrier(bus).await;
        events.drain().into_iter().map(|event| event.task).collect()
    }

    async fn the_task_event(
        bus: &residuum_bus::BusHandle,
        events: &mut residuum_bus::Subscriber<OutboundA2aTaskEvent>,
        what: &str,
    ) -> TrackedTask {
        let mut seen = task_events_now(bus, events).await;
        assert_eq!(seen.len(), 1, "{what}: {seen:?}");
        seen.pop().unwrap()
    }

    #[tokio::test]
    async fn each_unreachable_streak_publishes_one_event_at_its_threshold() {
        let dir = tempfile::tempdir().unwrap();
        let (messenger, bus) = messenger();
        let mut events = bus
            .subscribe::<_, OutboundA2aTaskEvent>(topics::Notification(NotifyName::from(
                SYSTEM_CHANNEL,
            )))
            .await
            .unwrap();
        let tracker = tracker_with_task(dir.path(), A2aClientHub::new_shared(), messenger).await;
        task_events_now(&bus, &mut events).await;

        for streak in 1..=2 {
            tracker.note_unreachable("t1", "connection refused").await;
            tracker.note_unreachable("t1", "connection refused").await;
            let started =
                the_task_event(&bus, &mut events, &format!("streak {streak} start")).await;
            assert!(started.first_unreachable_at.is_some());
            assert!(!started.unreachable_notified);

            {
                let mut store = tracker.store.write().await;
                let entry = store.tasks.get_mut("t1").unwrap();
                entry.first_unreachable_at = Some(Utc::now() - chrono::Duration::minutes(11));
            }
            tracker.note_unreachable("t1", "connection refused").await;
            tracker.note_unreachable("t1", "connection refused").await;
            let crossed =
                the_task_event(&bus, &mut events, &format!("streak {streak} threshold")).await;
            assert!(crossed.unreachable_notified);

            tracker.update_state("t1", "working", None, false).await;
            let cleared = the_task_event(&bus, &mut events, &format!("streak {streak} end")).await;
            assert!(cleared.first_unreachable_at.is_none());
        }
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
            .subscribe::<_, residuum_bus::MessageEvent>(topics::UserMessage)
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
