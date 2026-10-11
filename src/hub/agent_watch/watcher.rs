//! The per-agent watcher: one task per running agent that turns what happens
//! on the agent's bus into [`AgentChange`]s on the hub's feed.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::{Instant, Interval, MissedTickBehavior};
use tokio_util::sync::{CancellationToken, DropGuard};
use tracing::Instrument as _;

use super::changes::{
    AgentChange, AgentChangeFeed, AgentChangeKind, AgentSessionEvent, WatchedPath,
};
use crate::background::registry::SessionRegistry;
use crate::bus::{
    BusError, BusHandle, NotifyName, OutboundA2aTaskEvent, SYSTEM_CHANNEL, SessionAddress,
    SessionEvent, SessionEventKind, Subscriber, UserInboxAddedEvent, WorkspaceEvent, topics,
};
use crate::gateway::event_loop::{AgentControl, agent_span};
use crate::workspace::watch::{WatchHealth, WorkspaceChange};

/// How often an agent is recomputed while its file watcher is down.
const RECOMPUTE_WHILE_UNAVAILABLE: Duration = Duration::from_secs(60);

/// A running watcher of one agent. It stops when [`stop`](Self::stop)ped or
/// dropped.
pub(crate) struct AgentWatcher {
    cancel_on_drop: DropGuard,
    task: JoinHandle<()>,
}

impl AgentWatcher {
    /// Subscribe to the agent's bus and start watching it, publishing on
    /// `feed` under the agent's name.
    ///
    /// Call before the agent's event loop runs, so no session or inbox event
    /// is missed.
    ///
    /// # Errors
    /// Returns the bus error when a subscription can't be made; the agent
    /// then has no watcher.
    pub(crate) async fn attach(
        agent: &str,
        control: &AgentControl,
        feed: Arc<AgentChangeFeed>,
    ) -> Result<Self, BusError> {
        Self::attach_to(
            agent,
            &control.bus,
            control.workspace_watch_health.clone(),
            &control.session_registry,
            feed,
        )
        .await
    }

    async fn attach_to(
        agent: &str,
        bus: &BusHandle,
        workspace_health: watch::Receiver<WatchHealth>,
        registry: &SessionRegistry,
        feed: Arc<AgentChangeFeed>,
    ) -> Result<Self, BusError> {
        let subscriptions = Subscriptions::open(bus).await?;
        let mut state = WatchState::new(agent, feed);
        state.learn_live_sessions(registry);
        // Read after subscribing: a feed that went down before this point
        // published its `Unavailable` where nobody was listening, but had
        // already set its health.
        if *workspace_health.borrow() == WatchHealth::Off {
            state.workspace_unavailable();
        }
        let cancel = CancellationToken::new();
        let span = agent_span(agent);
        let task = span.in_scope(|| {
            crate::util::spawn_monitored(
                "agent-watcher",
                subscriptions
                    .run(state, cancel.clone())
                    .instrument(span.clone()),
            )
        });
        Ok(Self {
            cancel_on_drop: cancel.drop_guard(),
            task,
        })
    }

    /// Stop watching. Changes already queued are still published; once this
    /// returns, nothing more is.
    pub(crate) async fn stop(self) {
        let Self {
            cancel_on_drop,
            task,
        } = self;
        drop(cancel_on_drop);
        if let Err(e) = task.await {
            tracing::warn!(error = %e, "an agent's watcher ended abnormally");
        }
    }
}

/// The agent-bus subscriptions the watcher reads. Every one is lossless but
/// the workspace feed, and every one is drained for as long as the watcher
/// runs, whether or not anyone reads the hub's feed.
struct Subscriptions {
    sessions: Subscriber<SessionEvent>,
    outbound: Subscriber<OutboundA2aTaskEvent>,
    user_inbox: Subscriber<UserInboxAddedEvent>,
    workspace: Subscriber<WorkspaceEvent>,
}

/// What one subscription's `recv` means for the watcher loop.
enum Next<E> {
    Event(E),
    /// An event that couldn't be read; `Subscriber::recv` already logged it.
    Skip,
    /// The agent's broker shut down.
    Closed,
}

impl<E> Next<E> {
    fn of(received: Result<Option<E>, BusError>) -> Self {
        match received {
            Ok(Some(event)) => Self::Event(event),
            Ok(None) => Self::Closed,
            Err(_mismatch) => Self::Skip,
        }
    }
}

impl Subscriptions {
    async fn open(bus: &BusHandle) -> Result<Self, BusError> {
        Ok(Self {
            sessions: bus.subscribe(topics::Sessions).await?,
            outbound: bus
                .subscribe(topics::Notification(NotifyName::from(SYSTEM_CHANNEL)))
                .await?,
            user_inbox: bus.subscribe(topics::UserInbox).await?,
            workspace: bus.subscribe(topics::Workspace).await?,
        })
    }

    /// Read every subscription until `cancel` fires, then publish what is
    /// still queued.
    async fn run(mut self, mut state: WatchState, cancel: CancellationToken) {
        loop {
            tokio::select! {
                () = cancel.cancelled() => break,
                received = self.sessions.recv() => match Next::of(received) {
                    Next::Event(event) => state.on_session(event),
                    Next::Skip => {}
                    Next::Closed => { state.bus_closed(); break; }
                },
                received = self.outbound.recv() => match Next::of(received) {
                    Next::Event(event) => state.on_outbound_task(event),
                    Next::Skip => {}
                    Next::Closed => { state.bus_closed(); break; }
                },
                received = self.user_inbox.recv() => match Next::of(received) {
                    Next::Event(event) => state.on_user_inbox_added(event),
                    Next::Skip => {}
                    Next::Closed => { state.bus_closed(); break; }
                },
                received = self.workspace.recv() => match Next::of(received) {
                    Next::Event(event) => state.on_workspace(event),
                    Next::Skip => {}
                    Next::Closed => { state.bus_closed(); break; }
                },
                () = next_recompute(&mut state.recompute) => state.publish(AgentChangeKind::Resync),
            }
        }
        for event in self.sessions.drain() {
            state.on_session(event);
        }
        for event in self.outbound.drain() {
            state.on_outbound_task(event);
        }
        for event in self.user_inbox.drain() {
            state.on_user_inbox_added(event);
        }
        for event in self.workspace.drain() {
            state.on_workspace(event);
        }
    }
}

/// Waits for the next recompute tick, or forever while there is none.
async fn next_recompute(recompute: &mut Option<Interval>) {
    match recompute {
        Some(interval) => {
            interval.tick().await;
        }
        None => std::future::pending().await,
    }
}

/// What the watcher keeps between events.
struct WatchState {
    agent: String,
    feed: Arc<AgentChangeFeed>,
    /// The source label of each live session run, from its start event, so a
    /// session's later events can be matched to the label it started with.
    labels: HashMap<(SessionAddress, String), String>,
    /// Ticks every 60 seconds while the agent's file watcher is down, until
    /// its next batch or resync.
    recompute: Option<Interval>,
}

impl WatchState {
    fn new(agent: &str, feed: Arc<AgentChangeFeed>) -> Self {
        Self {
            agent: agent.to_string(),
            feed,
            labels: HashMap::new(),
            recompute: None,
        }
    }

    fn publish(&self, kind: AgentChangeKind) {
        self.feed.publish(&AgentChange {
            agent: self.agent.clone(),
            kind,
        });
    }

    /// Remember the label of every session already live, which started
    /// before the watcher was listening.
    fn learn_live_sessions(&mut self, registry: &SessionRegistry) {
        for info in registry.list_live() {
            self.labels
                .insert((info.address, info.run_id), info.source_label);
        }
    }

    /// Sessions: lifecycle events become changes; every event, lifecycle or
    /// turn, goes to the session relay.
    fn on_session(&mut self, event: SessionEvent) {
        let key = (event.address.clone(), event.run_id.clone());
        let change = match &event.kind {
            SessionEventKind::Started(info) => {
                self.labels.insert(key.clone(), info.source_label.clone());
                Some(AgentChangeKind::SessionStarted(info.clone()))
            }
            SessionEventKind::StateChanged(state) => Some(AgentChangeKind::SessionStateChanged {
                address: event.address.clone(),
                run_id: event.run_id.clone(),
                state: *state,
            }),
            SessionEventKind::Completed { status, episode_id } => {
                Some(AgentChangeKind::SessionCompleted {
                    address: event.address.clone(),
                    run_id: event.run_id.clone(),
                    status: status.clone(),
                    episode_id: episode_id.clone(),
                })
            }
            SessionEventKind::TurnStarted { .. }
            | SessionEventKind::TurnEnded { .. }
            | SessionEventKind::ToolCall(_)
            | SessionEventKind::ToolResult(_)
            | SessionEventKind::Intermediate { .. }
            | SessionEventKind::TurnUsage { .. }
            | SessionEventKind::Response { .. }
            | SessionEventKind::Error { .. }
            | SessionEventKind::MessageToMain { .. } => None,
        };
        if let Some(change) = change {
            self.publish(change);
        }
        let finished = matches!(event.kind, SessionEventKind::Completed { .. });
        let source_label = if finished {
            self.labels.remove(&key)
        } else {
            self.labels.get(&key).cloned()
        };
        self.feed.relay_session(AgentSessionEvent {
            agent: self.agent.clone(),
            source_label,
            event,
        });
    }

    fn on_outbound_task(&self, event: OutboundA2aTaskEvent) {
        self.publish(AgentChangeKind::OutboundTaskChanged(Box::new(event.task)));
    }

    fn on_user_inbox_added(&self, event: UserInboxAddedEvent) {
        self.publish(AgentChangeKind::UserInboxAdded {
            item_id: event.item_id,
        });
    }

    /// Workspace: a batch becomes one change per kind of watched file it
    /// touched. A resync is passed on, and a batch after an outage ends the
    /// recompute ticks.
    fn on_workspace(&mut self, event: WorkspaceEvent) {
        match event {
            WorkspaceEvent::Changed(changes) => {
                if self.recompute.take().is_some() {
                    tracing::info!(agent = %self.agent, "watching the agent's files again");
                    self.publish(AgentChangeKind::Resync);
                }
                for path in touched(&changes) {
                    self.publish(AgentChangeKind::WatchedPathChanged(path));
                }
            }
            WorkspaceEvent::Resync(reason) => {
                tracing::debug!(agent = %self.agent, ?reason, "the agent's file watcher may have missed changes");
                self.recompute = None;
                self.publish(AgentChangeKind::Resync);
            }
            WorkspaceEvent::Unavailable => self.workspace_unavailable(),
        }
    }

    /// The agent's file watcher is down: recompute every 60 seconds until a
    /// batch or resync arrives. Logs once per outage.
    fn workspace_unavailable(&mut self) {
        if self.recompute.is_some() {
            return;
        }
        tracing::warn!(
            agent = %self.agent,
            "couldn't watch the agent's files for changes; its inbox count, schedule and settings are refreshed every 60 seconds instead"
        );
        let mut every = tokio::time::interval_at(
            Instant::now() + RECOMPUTE_WHILE_UNAVAILABLE,
            RECOMPUTE_WHILE_UNAVAILABLE,
        );
        every.set_missed_tick_behavior(MissedTickBehavior::Delay);
        self.recompute = Some(every);
    }

    fn bus_closed(&self) {
        tracing::warn!(agent = %self.agent, "the agent's event bus closed; the hub stopped watching the agent");
    }
}

/// The kinds of watched file a batch of changes touched, in a fixed order.
fn touched(changes: &[WorkspaceChange]) -> BTreeSet<WatchedPath> {
    changes
        .iter()
        .filter_map(|change| watched_path(&change.path))
        .collect()
}

/// Which watched file a workspace-relative path is, if any.
///
/// A change to a directory stands for everything under it, so the
/// directories that hold watched files count too.
fn watched_path(path: &str) -> Option<WatchedPath> {
    let segments: Vec<&str> = path.split('/').collect();
    match segments.as_slice() {
        ["scheduled_actions.json"] => Some(WatchedPath::ScheduledActions),
        ["HEARTBEAT.yml"] => Some(WatchedPath::Heartbeat),
        ["pulse_state.json"] => Some(WatchedPath::PulseState),
        ["inbox"] | ["inbox", "user"] => Some(WatchedPath::UserInbox),
        ["inbox", "user", item] if Path::new(item).extension().is_some_and(|e| e == "json") => {
            Some(WatchedPath::UserInbox)
        }
        ["config"] | ["config", _] => Some(WatchedPath::Config),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::bus::{AgentResultStatus, SessionEventKind, ToolCallEvent};
    use crate::testing::{clock, wait};
    use crate::workspace::watch::{WorkspaceChangeKind, WorkspaceResyncReason};

    /// A watcher over a bare agent bus, with a receiver on the hub's feed.
    struct Rig {
        bus: BusHandle,
        feed: Arc<AgentChangeFeed>,
        changes: super::super::AgentChangeReceiver,
        watcher: Option<AgentWatcher>,
    }

    impl Rig {
        async fn new() -> Self {
            Self::with_health(WatchHealth::Native).await
        }

        async fn with_health(initial: WatchHealth) -> Self {
            let bus = crate::bus::spawn_broker();
            let feed = AgentChangeFeed::new();
            let changes = feed.subscribe();
            let (_health, health_rx) = watch::channel(initial);
            let watcher = AgentWatcher::attach_to(
                "scout",
                &bus,
                health_rx,
                &SessionRegistry::new(),
                Arc::clone(&feed),
            )
            .await
            .unwrap();
            Self {
                bus,
                feed,
                changes,
                watcher: Some(watcher),
            }
        }

        async fn publish<T, E>(&self, topic: T, event: E)
        where
            T: crate::bus::Topic + crate::bus::Carries<E>,
            E: Clone + Send + Sync + 'static,
        {
            self.bus.publisher().publish(topic, event).await.unwrap();
        }

        async fn workspace(&self, event: WorkspaceEvent) {
            self.publish(topics::Workspace, event).await;
        }

        /// The next change, if one arrives within `within` of paused time.
        async fn next_within(&mut self, within: Duration) -> Option<AgentChange> {
            clock::within(within, self.changes.recv()).await.flatten()
        }

        async fn next(&mut self) -> AgentChangeKind {
            let change = wait::next("a change to reach the stream", &mut self.changes).await;
            assert_eq!(change.agent, "scout");
            change.kind
        }

        async fn quiet(&mut self) {
            assert!(
                self.next_within(Duration::from_millis(100)).await.is_none(),
                "no further change"
            );
        }
    }

    fn batch(paths: &[&str]) -> WorkspaceEvent {
        WorkspaceEvent::Changed(
            paths
                .iter()
                .map(|path| WorkspaceChange {
                    path: (*path).to_string(),
                    kind: WorkspaceChangeKind::Modified,
                })
                .collect::<Vec<_>>()
                .into(),
        )
    }

    fn session_info(
        address: &str,
        run_id: &str,
        label: &str,
    ) -> crate::background::registry::SessionInfo {
        crate::background::registry::SessionInfo {
            address: SessionAddress::from(address),
            run_id: run_id.to_string(),
            category: crate::background::registry::SessionCategory::Spawned,
            trigger: crate::bus::EventTrigger::Agent,
            source_label: label.to_string(),
            state: crate::background::registry::SessionState::Forking,
            spawner: None,
            depth: 1,
            purpose: "look into it".to_string(),
            agent_skill: None,
            model_tier: crate::config::BackgroundModelTier::default(),
            conversation_target: None,
            started_at: chrono::Utc::now(),
            usage: crate::agent::usage::SessionUsageTotals::default(),
            overlap: None,
        }
    }

    fn session_event(address: &str, run_id: &str, kind: SessionEventKind) -> SessionEvent {
        SessionEvent {
            address: SessionAddress::from(address),
            run_id: run_id.to_string(),
            kind,
        }
    }

    #[test]
    fn only_the_files_the_hub_follows_are_watched() {
        for (path, expected) in [
            (
                "inbox/user/20260930_note.json",
                Some(WatchedPath::UserInbox),
            ),
            ("inbox/user", Some(WatchedPath::UserInbox)),
            ("inbox", Some(WatchedPath::UserInbox)),
            (
                "scheduled_actions.json",
                Some(WatchedPath::ScheduledActions),
            ),
            ("HEARTBEAT.yml", Some(WatchedPath::Heartbeat)),
            ("pulse_state.json", Some(WatchedPath::PulseState)),
            ("config/config.toml", Some(WatchedPath::Config)),
            ("config/mcp.json", Some(WatchedPath::Config)),
            ("config", Some(WatchedPath::Config)),
            ("inbox/user/attachments/20260930_note/plot.png", None),
            ("inbox/user/attachments", None),
            ("inbox/user/notes.md", None),
            ("inbox/agent/20260930_result.json", None),
            ("archive/inbox/user/20260930_note.json", None),
            ("config/nested/extra.toml", None),
            ("memory/recent_messages.json", None),
            ("projects/scheduled_actions.json", None),
            ("team/HEARTBEAT.yml", None),
            ("SOUL.md", None),
        ] {
            assert_eq!(watched_path(path), expected, "{path}");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_batch_is_one_change_per_kind_of_watched_file_it_touched() {
        let mut rig = Rig::new().await;
        rig.workspace(batch(&[
            "memory/observations.json",
            "inbox/user/a.json",
            "inbox/user/b.json",
            "config/config.toml",
            "HEARTBEAT.yml",
        ]))
        .await;
        for expected in [
            WatchedPath::UserInbox,
            WatchedPath::Heartbeat,
            WatchedPath::Config,
        ] {
            assert!(
                matches!(rig.next().await, AgentChangeKind::WatchedPathChanged(got) if got == expected),
                "{expected:?}"
            );
        }
        rig.quiet().await;

        rig.workspace(batch(&["memory/observations.json", "notes/todo.md"]))
            .await;
        rig.quiet().await;
    }

    #[tokio::test(start_paused = true)]
    async fn lifecycle_outbound_and_inbox_events_reach_the_stream() {
        let mut rig = Rig::new().await;
        let info = session_info("spawned-research-1", "run-1", "subagent:research");
        rig.publish(
            topics::Sessions,
            session_event(
                "spawned-research-1",
                "run-1",
                SessionEventKind::Started(Box::new(info)),
            ),
        )
        .await;
        assert!(matches!(
            rig.next().await,
            AgentChangeKind::SessionStarted(started)
                if started.address.as_ref() == "spawned-research-1" && started.source_label == "subagent:research"
        ));

        rig.publish(
            topics::Sessions,
            session_event(
                "spawned-research-1",
                "run-1",
                SessionEventKind::StateChanged(crate::background::registry::SessionState::Running),
            ),
        )
        .await;
        assert!(matches!(
            rig.next().await,
            AgentChangeKind::SessionStateChanged { run_id, state, .. }
                if run_id == "run-1" && state == crate::background::registry::SessionState::Running
        ));

        rig.publish(
            topics::Sessions,
            session_event(
                "spawned-research-1",
                "run-1",
                SessionEventKind::Completed {
                    status: AgentResultStatus::Completed,
                    episode_id: Some("ep-7".to_string()),
                },
            ),
        )
        .await;
        assert!(matches!(
            rig.next().await,
            AgentChangeKind::SessionCompleted { status: AgentResultStatus::Completed, episode_id: Some(episode), .. }
                if episode == "ep-7"
        ));

        let task = crate::a2a::TrackedTask {
            sender_address: "main".to_string(),
            agent: "laptop".to_string(),
            task_id: "t1".to_string(),
            context_id: "c1".to_string(),
            state: "working".to_string(),
            last_status_text: None,
            hop_count: 0,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            first_unreachable_at: None,
            unreachable_notified: true,
            notified_this_turn: false,
            stopped_by_user: false,
        };
        rig.publish(
            topics::Notification(NotifyName::from(SYSTEM_CHANNEL)),
            OutboundA2aTaskEvent { task },
        )
        .await;
        assert!(matches!(
            rig.next().await,
            AgentChangeKind::OutboundTaskChanged(outbound)
                if outbound.task_id == "t1" && outbound.unreachable_notified
        ));

        rig.publish(
            topics::UserInbox,
            UserInboxAddedEvent {
                item_id: "20260930_note".to_string(),
            },
        )
        .await;
        assert!(matches!(
            rig.next().await,
            AgentChangeKind::UserInboxAdded { item_id } if item_id == "20260930_note"
        ));
        rig.quiet().await;
    }

    #[tokio::test(start_paused = true)]
    async fn every_session_event_is_relayed_with_the_label_its_session_started_with() {
        let mut rig = Rig::new().await;
        let mut relay = rig.feed.subscribe_sessions();
        let address = "spawned-notes-1";
        let events = [
            SessionEventKind::Started(Box::new(session_info(address, "run-1", "artifact:notes"))),
            SessionEventKind::TurnStarted {
                turn_id: "turn-1".to_string(),
            },
            SessionEventKind::ToolCall(ToolCallEvent {
                correlation_id: "c".to_string(),
                tool_call_id: "call-1".to_string(),
                name: "read".to_string(),
                arguments: serde_json::json!({}),
                server: None,
            }),
            SessionEventKind::Response {
                turn_id: "turn-1".to_string(),
                content: "done".to_string(),
            },
            SessionEventKind::Completed {
                status: AgentResultStatus::Completed,
                episode_id: None,
            },
        ];
        for kind in events {
            rig.publish(topics::Sessions, session_event(address, "run-1", kind))
                .await;
        }
        for _ in 0..5 {
            let relayed = wait::next("a relayed session event", &mut relay).await;
            assert_eq!(relayed.agent, "scout");
            assert_eq!(relayed.source_label.as_deref(), Some("artifact:notes"));
        }
        // The run is over, so its label is forgotten; a later event of the
        // same run has none.
        rig.publish(
            topics::Sessions,
            session_event(
                address,
                "run-1",
                SessionEventKind::TurnEnded {
                    turn_id: "turn-2".to_string(),
                },
            ),
        )
        .await;
        let late = wait::next("the late relayed session event", &mut relay).await;
        assert_eq!(late.source_label, None);
        // Only the start and the completion were changes; the turn, tool and
        // response events were relayed and nothing else.
        assert!(matches!(
            rig.next().await,
            AgentChangeKind::SessionStarted(_)
        ));
        assert!(matches!(
            rig.next().await,
            AgentChangeKind::SessionCompleted { .. }
        ));
        rig.quiet().await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_workspace_resync_is_a_resync_whatever_its_reason() {
        let mut rig = Rig::new().await;
        for reason in [
            WorkspaceResyncReason::Overflow,
            WorkspaceResyncReason::WatcherRestarted,
        ] {
            rig.workspace(WorkspaceEvent::Resync(reason)).await;
            assert!(
                matches!(rig.next().await, AgentChangeKind::Resync),
                "{reason:?}"
            );
        }
        rig.quiet().await;
    }

    #[tokio::test(start_paused = true)]
    async fn an_unavailable_watcher_recomputes_every_minute_until_its_next_batch() {
        let mut rig = Rig::new().await;
        rig.workspace(WorkspaceEvent::Unavailable).await;
        // A second notice in the same outage changes nothing.
        rig.workspace(WorkspaceEvent::Unavailable).await;
        rig.quiet().await;

        assert!(
            rig.next_within(Duration::from_secs(59)).await.is_none(),
            "nothing before the first minute is up"
        );
        for minute in 1..=3 {
            assert!(
                matches!(
                    rig.next_within(Duration::from_secs(61))
                        .await
                        .map(|c| c.kind),
                    Some(AgentChangeKind::Resync)
                ),
                "recompute {minute}"
            );
        }

        // The first batch after the outage recomputes once more, then the
        // ticks stop.
        rig.workspace(batch(&["HEARTBEAT.yml"])).await;
        assert!(matches!(rig.next().await, AgentChangeKind::Resync));
        assert!(matches!(
            rig.next().await,
            AgentChangeKind::WatchedPathChanged(WatchedPath::Heartbeat)
        ));
        assert!(rig.next_within(Duration::from_secs(300)).await.is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn a_resync_ends_the_recompute_ticks() {
        let mut rig = Rig::new().await;
        rig.workspace(WorkspaceEvent::Unavailable).await;
        rig.quiet().await;
        rig.workspace(WorkspaceEvent::Resync(
            WorkspaceResyncReason::WatcherRestarted,
        ))
        .await;
        assert!(matches!(rig.next().await, AgentChangeKind::Resync));
        assert!(rig.next_within(Duration::from_secs(300)).await.is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn a_file_watcher_that_was_already_down_when_the_watcher_attached_is_recomputed() {
        let mut rig = Rig::with_health(WatchHealth::Off).await;
        assert!(matches!(
            rig.next_within(Duration::from_secs(61))
                .await
                .map(|c| c.kind),
            Some(AgentChangeKind::Resync)
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn the_watcher_keeps_draining_while_nothing_reads_its_output() {
        let mut rig = Rig::new().await;
        // More than the bus's stuck-consumer threshold: a watcher that
        // stopped reading would show up as a growing backlog, and its
        // output would stall.
        let total = 12_000;
        for n in 0..total {
            rig.publish(
                topics::UserInbox,
                UserInboxAddedEvent {
                    item_id: n.to_string(),
                },
            )
            .await;
        }
        rig.publish(
            topics::UserInbox,
            UserInboxAddedEvent {
                item_id: "last".to_string(),
            },
        )
        .await;
        // Nothing has read the feed so far, and nothing was lost.
        for n in 0..total {
            assert!(
                matches!(rig.next().await, AgentChangeKind::UserInboxAdded { item_id } if item_id == n.to_string()),
                "change {n}"
            );
        }
        assert!(matches!(
            rig.next().await,
            AgentChangeKind::UserInboxAdded { item_id } if item_id == "last"
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn the_watcher_drains_its_subscriptions_with_no_one_subscribed_to_the_feed() {
        let bus = crate::bus::spawn_broker();
        let feed = AgentChangeFeed::new();
        let (_health, health_rx) = watch::channel(WatchHealth::Native);
        let watcher = AgentWatcher::attach_to(
            "scout",
            &bus,
            health_rx,
            &SessionRegistry::new(),
            Arc::clone(&feed),
        )
        .await
        .unwrap();
        for n in 0..12_000 {
            bus.publisher()
                .publish(
                    topics::UserInbox,
                    UserInboxAddedEvent {
                        item_id: n.to_string(),
                    },
                )
                .await
                .unwrap();
        }
        // A subscriber made now hears every change from here on: if the
        // watcher has read all of the above (with no one listening to any of
        // it) and is still reading, the change below reaches it.
        let mut late = feed.subscribe();
        bus.publisher()
            .publish(
                topics::UserInbox,
                UserInboxAddedEvent {
                    item_id: "after".to_string(),
                },
            )
            .await
            .unwrap();
        wait::next_matching(
            "the watcher to read everything and keep reading",
            &mut late,
            |change| {
                matches!(
                    change.kind,
                    AgentChangeKind::UserInboxAdded { ref item_id } if item_id == "after"
                )
            },
        )
        .await;
        watcher.stop().await;
    }

    #[tokio::test(start_paused = true)]
    async fn nothing_is_published_once_the_watcher_has_stopped() {
        let mut rig = Rig::new().await;
        rig.publish(
            topics::UserInbox,
            UserInboxAddedEvent {
                item_id: "before".to_string(),
            },
        )
        .await;
        assert!(matches!(
            rig.next().await,
            AgentChangeKind::UserInboxAdded { item_id } if item_id == "before"
        ));

        rig.watcher.take().unwrap().stop().await;
        rig.publish(
            topics::UserInbox,
            UserInboxAddedEvent {
                item_id: "after".to_string(),
            },
        )
        .await;
        rig.workspace(batch(&["HEARTBEAT.yml"])).await;
        rig.publish(
            topics::Sessions,
            session_event(
                "spawned-x",
                "run-1",
                SessionEventKind::StateChanged(crate::background::registry::SessionState::Idle),
            ),
        )
        .await;
        rig.quiet().await;
    }

    #[tokio::test(start_paused = true)]
    async fn changes_queued_when_the_watcher_stops_are_still_published() {
        let mut rig = Rig::new().await;
        // A second subscriber on the topic, registered after the watcher's,
        // has an event only once the broker has already queued it for the
        // watcher.
        let mut barrier: Subscriber<UserInboxAddedEvent> =
            rig.bus.subscribe(topics::UserInbox).await.unwrap();
        for n in 0..3 {
            rig.publish(
                topics::UserInbox,
                UserInboxAddedEvent {
                    item_id: n.to_string(),
                },
            )
            .await;
        }
        for _ in 0..3 {
            barrier.recv().await.unwrap();
        }

        rig.watcher.take().unwrap().stop().await;
        for n in 0..3 {
            assert!(
                matches!(rig.next().await, AgentChangeKind::UserInboxAdded { item_id } if item_id == n.to_string()),
                "change {n}"
            );
        }
        rig.quiet().await;
    }
}
