//! WebSocket bus subscriber — translates typed bus events to `ServerMessage` frames.

use crate::turn_journal::{JournaledEvent, TurnJournal, TurnRecord};
use residuum_bus::{
    EndpointName, ErrorEvent, InlineOutputEvent, MainConversationEvent, NoticeEvent, NotifyName,
    OutboundA2aTaskEvent, PostTurnActivityEvent, PostTurnActivityKind, ResponseEvent, SessionEvent,
    SessionStartFailedEvent, Subscriber, TurnUsageEvent, WorkbenchEvent, WorkspaceEvent, topics,
};
use residuum_hub::file_server::FileRegistry;
use residuum_protocol::frames::{
    session_event_to_server_message, session_start_failed_frame, workspace_frame,
};
use residuum_protocol::{QueuedMessage, ServerMessage, SnapshotFrame, TurnInProgress};
use tokio::sync::mpsc;

use residuum_workspace::watch::WatchSet;

/// The frame for one event of the main agent's conversation.
#[expect(
    clippy::too_many_lines,
    reason = "one arm per conversation event, each a field-for-field move into its frame"
)]
pub(crate) fn main_conversation_frame(event: MainConversationEvent) -> ServerMessage {
    match event {
        MainConversationEvent::TurnStarted { turn_id, origin } => ServerMessage::TurnStarted {
            reply_to: turn_id,
            origin,
        },
        MainConversationEvent::TurnEnded { turn_id } => {
            ServerMessage::TurnEnded { reply_to: turn_id }
        }
        MainConversationEvent::UserMessage {
            id,
            turn_id,
            content,
            images,
            sender,
            agent_sender,
            endpoint,
        } => ServerMessage::UserMessage {
            id,
            turn_id,
            content,
            images,
            sender,
            agent_sender,
            endpoint,
        },
        MainConversationEvent::MessageQueued {
            id,
            content,
            images,
            sender,
            endpoint,
        } => ServerMessage::MessageQueued {
            message: QueuedMessage {
                id,
                content,
                images,
                sender,
                endpoint,
            },
        },
        MainConversationEvent::MessageWithdrawn { id } => ServerMessage::MessageWithdrawn { id },
        MainConversationEvent::ToolCall { call, event } => ServerMessage::ToolCall {
            reply_to: event.correlation_id,
            call,
            id: event.tool_call_id,
            name: event.name,
            arguments: event.arguments,
            server: event.server,
        },
        MainConversationEvent::ToolResult(result) => ServerMessage::ToolResult {
            reply_to: result.correlation_id,
            tool_call_id: result.tool_call_id,
            name: result.name,
            output: result.output,
            is_error: result.is_error,
            auto_mode: result.auto_mode,
        },
        MainConversationEvent::TextDelta {
            turn_id,
            call,
            text,
        } => ServerMessage::TextDelta {
            reply_to: turn_id,
            call,
            text,
        },
        MainConversationEvent::ThinkingDelta {
            turn_id,
            call,
            text,
        } => ServerMessage::ThinkingDelta {
            reply_to: turn_id,
            call,
            text,
        },
        MainConversationEvent::StreamRestart { turn_id, call } => ServerMessage::StreamRestart {
            reply_to: turn_id,
            call,
        },
        MainConversationEvent::Thinking {
            turn_id,
            call,
            content,
        } => ServerMessage::Thinking {
            reply_to: turn_id,
            call,
            content,
        },
        MainConversationEvent::Intermediate {
            turn_id,
            call,
            content,
        } => ServerMessage::BroadcastResponse {
            reply_to: turn_id,
            call,
            content,
        },
        MainConversationEvent::Response {
            turn_id,
            call,
            endpoint,
            content,
        } => ServerMessage::Response {
            reply_to: turn_id,
            call,
            endpoint,
            content,
        },
        MainConversationEvent::TurnUsage(usage) => turn_usage_frame(usage),
    }
}

/// The turn in flight as a `turn_snapshot` frame carries it.
fn turn_in_progress(record: TurnRecord) -> TurnInProgress {
    let stamp =
        |at: chrono::DateTime<chrono::Utc>| at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    TurnInProgress {
        reply_to: record.turn_id,
        started_at: stamp(record.started_at),
        now: stamp(chrono::Utc::now()),
        frames: record
            .entries
            .into_iter()
            .map(|entry| SnapshotFrame {
                at: stamp(entry.at),
                frame: main_conversation_frame(entry.event),
            })
            .collect(),
    }
}

/// The frame for a background post-turn cycle (see
/// `crate::post_turn`) starting or finishing.
fn post_turn_activity_frame(event: PostTurnActivityEvent) -> ServerMessage {
    ServerMessage::PostTurnActivity {
        kind: match event.kind {
            PostTurnActivityKind::Memory => residuum_protocol::PostTurnActivityKind::Memory,
            PostTurnActivityKind::Subconscious => {
                residuum_protocol::PostTurnActivityKind::Subconscious
            }
        },
        active: event.active,
    }
}

/// The frame for the main agent's turn-usage progress.
fn turn_usage_frame(usage: TurnUsageEvent) -> ServerMessage {
    ServerMessage::TurnUsage {
        reply_to: usage.correlation_id,
        output_tokens: usage.output_tokens,
        has_usage: usage.has_usage,
        tool_calls: usage.tool_calls,
        session_totals: usage.session_totals,
    }
}

/// Convert a message the agent posted to this endpoint into the appropriate
/// `ServerMessage`.
///
/// If the post carries a file attachment, registers it with the file
/// registry and returns a `FileAttachment` frame; otherwise returns a plain
/// `Response` frame delivered to `endpoint`. Extracted from
/// `WsSubscribers::recv` to keep the select loop within clippy's
/// `too_many_lines` budget.
async fn response_to_server_message(
    registry: &FileRegistry,
    endpoint: &str,
    resp: ResponseEvent,
) -> ServerMessage {
    if let Some(att) = resp.attachment {
        let url = registry
            .url_for(
                att.path.clone(),
                att.mime_type.clone(),
                att.filename.clone(),
            )
            .await;
        let caption = if resp.content.is_empty() {
            None
        } else {
            Some(resp.content.clone())
        };
        ServerMessage::FileAttachment {
            reply_to: resp.correlation_id,
            filename: att.filename,
            mime_type: att.mime_type,
            size: att.size,
            url,
            caption,
        }
    } else {
        ServerMessage::Response {
            reply_to: resp.correlation_id,
            call: None,
            endpoint: endpoint.to_string(),
            content: resp.content,
        }
    }
}

/// Typed subscribers for a single WebSocket connection.
pub struct WsSubscribers {
    /// Every main-agent turn, whatever endpoint started it, in order, as the
    /// turn journal relays it: the source of all the main conversation's
    /// frames — turn lifecycle, user messages, tool activity, thinking,
    /// streamed text, intermediate text, replies and usage.
    main: mpsc::UnboundedReceiver<JournaledEvent>,
    /// The main conversation's turn in flight, for `turn_snapshot` frames.
    journal: TurnJournal,
    /// The last relayed event a `turn_snapshot` already sent covers; relayed
    /// events up to it are dropped so none is shown twice.
    covered_through: u64,
    /// Requests for a `turn_snapshot`, from the connection's read loop.
    resync_rx: mpsc::UnboundedReceiver<()>,
    resync_tx: mpsc::UnboundedSender<()>,
    /// Messages posted to this endpoint with `send_message`, files included.
    /// Nothing else is published to the web UI's endpoint: a turn's reply
    /// arrives through `main`.
    pub response: Subscriber<ResponseEvent>,
    /// The endpoint `response` listens on, named in the frames it yields.
    endpoint: EndpointName,
    /// Background post-turn cycle start/finish, for the quiet activity
    /// indicator — see `crate::post_turn`.
    pub post_turn_activity: Subscriber<PostTurnActivityEvent>,
    pub notice: Subscriber<NoticeEvent>,
    pub inline_output: Subscriber<InlineOutputEvent>,
    pub error: Subscriber<ErrorEvent>,
    /// Agent session lifecycle and turn events, forwarded as the
    /// `session_*` frames. Main-agent frames never come from here.
    pub session: Subscriber<SessionEvent>,
    /// Owner-started sessions that couldn't be forked, forwarded as
    /// `session_start_failed` frames.
    pub session_start_failed: Subscriber<SessionStartFailedEvent>,
    /// Tasks sent to remote agents, for Activity's Running now.
    pub outbound_a2a: Subscriber<OutboundA2aTaskEvent>,
    /// Workbench artifact file changes, so an open artifact page reloads live.
    pub workbench: Subscriber<WorkbenchEvent>,
    /// The agent's own workspace change feed, filtered by `watch_set`.
    pub workspace: Subscriber<WorkspaceEvent>,
    /// The hub's team change feed (paths under `team/`), filtered by
    /// `watch_set`.
    pub team_workspace: Subscriber<WorkspaceEvent>,
    /// The prefixes this connection watches, replaced by its
    /// `watch_workspace` frames.
    pub watch_set: tokio::sync::watch::Receiver<WatchSet>,
    pub file_registry: residuum_hub::file_server::FileRegistry,
}

impl WsSubscribers {
    /// Create all typed subscribers for a WebSocket connection.
    ///
    /// # Errors
    ///
    /// Returns `BusError` if any subscription fails.
    pub async fn new(
        bus_handle: &residuum_bus::BusHandle,
        team_bus: &residuum_bus::BusHandle,
        ep: EndpointName,
        file_registry: residuum_hub::file_server::FileRegistry,
        watch_set: tokio::sync::watch::Receiver<WatchSet>,
        journal: &TurnJournal,
    ) -> Result<Self, residuum_bus::BusError> {
        let system_topic = || topics::Notification(NotifyName::from(residuum_bus::SYSTEM_CHANNEL));
        let (resync_tx, resync_rx) = mpsc::unbounded_channel();
        Ok(Self {
            main: journal.follow(),
            journal: journal.clone(),
            covered_through: 0,
            resync_rx,
            resync_tx,
            response: bus_handle.subscribe(topics::Endpoint(ep.clone())).await?,
            endpoint: ep,
            post_turn_activity: bus_handle.subscribe(system_topic()).await?,
            notice: bus_handle.subscribe(system_topic()).await?,
            inline_output: bus_handle.subscribe(system_topic()).await?,
            error: bus_handle.subscribe(system_topic()).await?,
            session: bus_handle.subscribe(topics::Sessions).await?,
            session_start_failed: bus_handle.subscribe(topics::Sessions).await?,
            outbound_a2a: bus_handle.subscribe(system_topic()).await?,
            workbench: bus_handle.subscribe(topics::Workbench).await?,
            workspace: bus_handle.subscribe(topics::Workspace).await?,
            team_workspace: team_bus.subscribe(topics::Workspace).await?,
            watch_set,
            file_registry,
        })
    }

    /// Where the connection's read loop asks for a `turn_snapshot` frame,
    /// which then arrives in order among the others from [`Self::recv`].
    #[must_use]
    pub fn resync_requests(&self) -> mpsc::UnboundedSender<()> {
        self.resync_tx.clone()
    }

    /// The turn in flight as of now; the relayed events it covers are dropped.
    fn turn_snapshot_frame(&mut self) -> ServerMessage {
        let snapshot = self.journal.snapshot();
        self.covered_through = snapshot.through;
        ServerMessage::TurnSnapshot {
            turn: snapshot.turn.map(turn_in_progress),
            queued: snapshot.queued,
        }
    }

    /// Receive the next server message from any subscribed topic.
    ///
    /// Returns `None` when all subscribers have closed.
    pub async fn recv(&mut self) -> Option<ServerMessage> {
        loop {
            let msg = tokio::select! {
                event = self.main.recv() => match event {
                    Some(relayed) if relayed.seq <= self.covered_through => None,
                    Some(relayed) => Some(main_conversation_frame(relayed.event)),
                    None => return None,
                },
                Some(()) = self.resync_rx.recv() => Some(self.turn_snapshot_frame()),
                event = self.response.recv() => {
                    match event {
                        Ok(Some(resp)) => Some(
                            response_to_server_message(
                                &self.file_registry,
                                self.endpoint.as_ref(),
                                resp,
                            )
                            .await,
                        ),
                        _ => return None,
                    }
                }
                event = self.post_turn_activity.recv() => match event {
                    Ok(Some(activity)) => Some(post_turn_activity_frame(activity)),
                    _ => return None,
                },
                event = self.notice.recv() => match event {
                    Ok(Some(NoticeEvent { message, reload })) => Some(ServerMessage::Notice {
                        message,
                        reload: reload.map(|report| report.outcome),
                        reload_of: reload.map(|report| report.of),
                    }),
                    _ => return None,
                },
                event = self.inline_output.recv() => match event {
                    Ok(Some(InlineOutputEvent { message })) => {
                        Some(ServerMessage::InlineOutput { message })
                    }
                    _ => return None,
                },
                event = self.session.recv() => match event {
                    Ok(Some(session_event)) => Some(session_event_to_server_message(session_event)),
                    _ => return None,
                },
                event = self.session_start_failed.recv() => match event {
                    Ok(Some(failed)) => Some(session_start_failed_frame(failed)),
                    _ => return None,
                },
                event = self.outbound_a2a.recv() => match event {
                    Ok(Some(OutboundA2aTaskEvent { task })) => {
                        Some(ServerMessage::SessionOutboundA2aTask { task: (&task).into() })
                    }
                    _ => return None,
                },
                event = self.workbench.recv() => {
                    match event {
                        Ok(Some(WorkbenchEvent::Updated { name })) => {
                            Some(ServerMessage::ArtifactUpdated { name })
                        }
                        Ok(Some(WorkbenchEvent::Removed { name })) => {
                            Some(ServerMessage::ArtifactRemoved { name })
                        }
                        _ => return None,
                    }
                }
                event = self.workspace.recv() => {
                    match event {
                        Ok(Some(workspace_event)) => {
                            workspace_frame(&self.watch_set.borrow(), workspace_event)
                        }
                        _ => return None,
                    }
                }
                event = self.team_workspace.recv() => {
                    match event {
                        Ok(Some(workspace_event)) => {
                            workspace_frame(&self.watch_set.borrow(), workspace_event)
                        }
                        _ => return None,
                    }
                }
                event = self.error.recv() => {
                    match event {
                        Ok(Some(ErrorEvent { correlation_id, message, details })) => {
                            Some(ServerMessage::Error {
                                reply_to: Some(correlation_id),
                                message,
                                details,
                            })
                        }
                        _ => return None,
                    }
                }
            };

            if let Some(msg) = msg {
                return Some(msg);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;
    use residuum_bus::workspace_change::{WorkspaceChange, WorkspaceChangeKind};
    use residuum_bus::{ReloadOutcome, ReloadedFiles, ToolCallEvent, ToolResultEvent};

    fn ts() -> chrono::NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 3, 13)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
    }

    /// A broker with a `WsSubscribers` on the `ws` endpoint.
    async fn subscribed() -> (residuum_bus::BusHandle, WsSubscribers) {
        let handle = residuum_bus::spawn_broker();
        let subs = WsSubscribers::new(
            &handle,
            &handle,
            EndpointName::from("ws"),
            residuum_hub::file_server::FileRegistry::new("scout"),
            no_watch_set(),
            &TurnJournal::spawn(&handle).await.unwrap(),
        )
        .await
        .unwrap();
        (handle, subs)
    }

    fn delta(text: &str) -> MainConversationEvent {
        MainConversationEvent::TextDelta {
            turn_id: "t1".into(),
            call: 0,
            text: text.into(),
        }
    }

    #[tokio::test]
    async fn a_turn_snapshot_holds_the_turn_so_far_and_nothing_after_it_repeats() {
        let handle = residuum_bus::spawn_broker();
        let journal = TurnJournal::spawn(&handle).await.unwrap();
        let mut subs = WsSubscribers::new(
            &handle,
            &handle,
            EndpointName::from("ws"),
            residuum_hub::file_server::FileRegistry::new("scout"),
            no_watch_set(),
            &journal,
        )
        .await
        .unwrap();
        // Wait for the journal itself rather than for time to pass.
        let mut seen = journal.follow();
        publish_main(
            &handle,
            MainConversationEvent::TurnStarted {
                turn_id: "t1".into(),
                origin: origin("ws"),
            },
        )
        .await;
        publish_main(&handle, delta("Hel")).await;
        publish_main(&handle, delta("lo")).await;
        for _ in 0..3 {
            seen.recv().await.unwrap();
        }
        subs.resync_requests().send(()).unwrap();
        publish_main(&handle, delta(" there")).await;

        // Frames relayed before the request may come first; the snapshot
        // covers them, and only what came after it follows.
        let turn = loop {
            if let ServerMessage::TurnSnapshot { turn, .. } = subs.recv().await.unwrap() {
                break turn.unwrap();
            }
        };
        assert_eq!(turn.reply_to, "t1");
        let frames: Vec<String> = turn
            .frames
            .iter()
            .map(|f| serde_json::to_value(&f.frame).unwrap().to_string())
            .collect();
        let [_, streamed] = frames.as_slice() else {
            panic!("expected the turn's start and its text, got {frames:?}");
        };
        assert!(streamed.contains("\"text\":\"Hello\""), "{frames:?}");
        let next = subs.recv().await.unwrap();
        assert!(
            matches!(&next, ServerMessage::TextDelta { text, .. } if text == " there"),
            "expected the delta after the snapshot, got {next:?}"
        );
    }

    #[tokio::test]
    async fn a_turn_snapshot_with_no_turn_running_names_none() {
        let (_handle, mut subs) = subscribed().await;
        subs.resync_requests().send(()).unwrap();
        assert!(matches!(
            subs.recv().await.unwrap(),
            ServerMessage::TurnSnapshot { turn: None, .. }
        ));
    }

    #[tokio::test]
    async fn a_teammates_message_reaches_the_socket_with_its_sender_live_and_in_a_snapshot() {
        let (handle, mut subs) = subscribed().await;
        let teammate = residuum_inference::AgentSender {
            address: "agent:atlas".into(),
            category: "teammate".into(),
        };
        publish_main(
            &handle,
            MainConversationEvent::UserMessage {
                id: "bg-1".into(),
                turn_id: "bg-1".into(),
                content: "[Message from teammate agent:atlas, not the user.]\nstatus?".into(),
                images: Vec::new(),
                sender: None,
                agent_sender: Some(teammate.clone()),
                endpoint: "background".into(),
            },
        )
        .await;
        publish_main(
            &handle,
            MainConversationEvent::TurnStarted {
                turn_id: "bg-1".into(),
                origin: origin("background"),
            },
        )
        .await;

        let live = subs.recv().await.unwrap();
        assert!(
            matches!(
                &live,
                ServerMessage::UserMessage { id, agent_sender: Some(sender), .. }
                    if id == "bg-1" && *sender == teammate
            ),
            "the message goes out live with its sender, got {live:?}"
        );
        assert!(matches!(
            subs.recv().await.unwrap(),
            ServerMessage::TurnStarted { .. }
        ));

        // A page that opens now is shown it too.
        subs.resync_requests().send(()).unwrap();
        let ServerMessage::TurnSnapshot {
            turn: Some(turn), ..
        } = subs.recv().await.unwrap()
        else {
            panic!("expected a snapshot of the running turn");
        };
        let opening = turn.frames.first().map(|f| &f.frame);
        assert!(
            matches!(
                opening,
                Some(ServerMessage::UserMessage { id, agent_sender: Some(sender), .. })
                    if id == "bg-1" && *sender == teammate
            ),
            "the snapshot opens with the teammate's message and its sender, got {opening:?}"
        );
    }

    async fn publish_main(handle: &residuum_bus::BusHandle, event: MainConversationEvent) {
        handle
            .publisher()
            .publish(topics::MainConversation, event)
            .await
            .unwrap();
    }

    fn origin(endpoint: &str) -> residuum_bus::TurnOrigin {
        residuum_bus::TurnOrigin {
            endpoint: endpoint.into(),
            sender: None,
            visibility: residuum_memory::types::Visibility::User,
        }
    }

    /// A turn that started on Telegram, thought, spoke, called a tool and
    /// replied, as the events its loop publishes.
    fn a_telegram_turn() -> Vec<MainConversationEvent> {
        vec![
            MainConversationEvent::UserMessage {
                id: "t1".into(),
                turn_id: "t1".into(),
                content: "what time is it".into(),
                images: Vec::new(),
                sender: None,
                agent_sender: None,
                endpoint: "telegram".into(),
            },
            MainConversationEvent::TurnStarted {
                turn_id: "t1".into(),
                origin: origin("telegram"),
            },
            MainConversationEvent::TextDelta {
                turn_id: "t1".into(),
                call: 0,
                text: "Let me ".into(),
            },
            MainConversationEvent::ThinkingDelta {
                turn_id: "t1".into(),
                call: 0,
                text: "hm".into(),
            },
            MainConversationEvent::StreamRestart {
                turn_id: "t1".into(),
                call: 0,
            },
            MainConversationEvent::Thinking {
                turn_id: "t1".into(),
                call: 0,
                content: "hmm".into(),
            },
            MainConversationEvent::Intermediate {
                turn_id: "t1".into(),
                call: 0,
                content: "Checking the clock.".into(),
            },
            MainConversationEvent::ToolCall {
                call: 0,
                event: ToolCallEvent {
                    correlation_id: "t1".into(),
                    tool_call_id: "tc1".into(),
                    name: "exec".into(),
                    arguments: serde_json::json!({"command": "date"}),
                    server: None,
                },
            },
            MainConversationEvent::ToolResult(ToolResultEvent {
                correlation_id: "t1".into(),
                tool_call_id: "tc1".into(),
                name: "exec".into(),
                output: "noon".into(),
                is_error: false,
                auto_mode: None,
            }),
            MainConversationEvent::Response {
                turn_id: "t1".into(),
                call: Some(1),
                endpoint: "telegram".into(),
                content: "It is noon.".into(),
            },
            MainConversationEvent::TurnEnded {
                turn_id: "t1".into(),
            },
        ]
    }

    #[tokio::test]
    async fn a_turn_maps_to_its_frames_in_the_order_it_happened() {
        let (handle, mut subs) = subscribed().await;
        for event in a_telegram_turn() {
            publish_main(&handle, event).await;
        }

        let mut frames = Vec::new();
        for _ in 0..11 {
            frames.push(subs.recv().await.unwrap());
        }
        let kinds: Vec<String> = frames
            .iter()
            .map(|frame| {
                serde_json::to_value(frame)
                    .unwrap()
                    .get("type")
                    .and_then(serde_json::Value::as_str)
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "user_message",
                "turn_started",
                "text_delta",
                "thinking_delta",
                "stream_restart",
                "thinking",
                "broadcast_response",
                "tool_call",
                "tool_result",
                "response",
                "turn_ended",
            ],
            "one channel keeps a turn's frames in the order they were published"
        );
    }

    #[tokio::test]
    async fn a_turns_frames_carry_its_turn_call_and_origin() {
        let (handle, mut subs) = subscribed().await;
        for event in a_telegram_turn() {
            publish_main(&handle, event).await;
        }
        let mut frames = Vec::new();
        for _ in 0..11 {
            frames.push(subs.recv().await.unwrap());
        }
        let [
            user,
            started,
            _text,
            _thinking_delta,
            restart,
            thinking,
            intermediate,
            tool_call,
            tool_result,
            response,
            ended,
        ] = <[ServerMessage; 11]>::try_from(frames).unwrap();

        assert!(matches!(
            user,
            ServerMessage::UserMessage { id, turn_id, endpoint, .. }
                if id == "t1" && turn_id == "t1" && endpoint == "telegram"
        ));
        assert!(matches!(
            started,
            ServerMessage::TurnStarted { reply_to, origin }
                if reply_to == "t1" && origin.endpoint == "telegram"
        ));
        assert!(matches!(
            restart,
            ServerMessage::StreamRestart { reply_to, call: 0 } if reply_to == "t1"
        ));
        assert!(matches!(
            thinking,
            ServerMessage::Thinking { reply_to, call: 0, content }
                if reply_to == "t1" && content == "hmm"
        ));
        assert!(matches!(
            intermediate,
            ServerMessage::BroadcastResponse { reply_to, call: 0, content }
                if reply_to == "t1" && content == "Checking the clock."
        ));
        assert!(matches!(
            tool_call,
            ServerMessage::ToolCall { reply_to, call: 0, id, name, .. }
                if reply_to == "t1" && id == "tc1" && name == "exec"
        ));
        assert!(matches!(
            tool_result,
            ServerMessage::ToolResult { reply_to, tool_call_id, output, is_error: false, .. }
                if reply_to == "t1" && tool_call_id == "tc1" && output == "noon"
        ));
        assert!(matches!(
            response,
            ServerMessage::Response { reply_to, call: Some(1), endpoint, content }
                if reply_to == "t1" && endpoint == "telegram" && content == "It is noon."
        ));
        assert!(matches!(ended, ServerMessage::TurnEnded { reply_to } if reply_to == "t1"));
    }

    #[tokio::test]
    async fn turn_usage_maps_to_server_message() {
        let (handle, mut subs) = subscribed().await;

        let mut totals = residuum_bus::usage::SessionUsageTotals::default();
        totals.accumulate(Some(residuum_inference::Usage {
            input_tokens: 100,
            output_tokens: 20,
            cache_creation_tokens: None,
            cache_read_tokens: None,
        }));

        publish_main(
            &handle,
            MainConversationEvent::TurnUsage(residuum_bus::TurnUsageEvent {
                correlation_id: "c1".into(),
                output_tokens: 20,
                has_usage: true,
                tool_calls: 4,
                session_totals: Some(totals),
            }),
        )
        .await;

        let msg = subs.recv().await.unwrap();
        assert!(
            matches!(
                &msg,
                ServerMessage::TurnUsage { reply_to, output_tokens: 20, has_usage: true, tool_calls: 4, session_totals: Some(t) }
                    if reply_to == "c1" && *t == totals
            ),
            "TurnUsageEvent should map to ServerMessage::TurnUsage: {msg:?}"
        );
    }

    #[tokio::test]
    async fn a_message_posted_to_the_endpoint_maps_to_a_response_outside_any_turn() {
        let (handle, mut subs) = subscribed().await;

        handle
            .publisher()
            .publish(
                topics::Endpoint(EndpointName::from("ws")),
                ResponseEvent {
                    correlation_id: String::new(),
                    content: "heads up".into(),
                    timestamp: ts(),
                    attachment: None,
                    conversation: None,
                },
            )
            .await
            .unwrap();

        let msg = subs.recv().await.unwrap();
        assert!(
            matches!(
                &msg,
                ServerMessage::Response { reply_to, call: None, endpoint, content }
                    if reply_to.is_empty() && endpoint == "ws" && content == "heads up"
            ),
            "{msg:?}"
        );
    }

    #[tokio::test]
    async fn inline_output_maps_to_server_message() {
        let (handle, mut subs) = subscribed().await;

        handle
            .publisher()
            .publish(
                topics::Notification(NotifyName::from(residuum_bus::SYSTEM_CHANNEL)),
                residuum_bus::InlineOutputEvent {
                    message: "[context]\n  identity: ~100 tokens".into(),
                },
            )
            .await
            .unwrap();

        let msg = subs.recv().await.unwrap();
        assert!(matches!(
            msg,
            ServerMessage::InlineOutput { message }
                if message == "[context]\n  identity: ~100 tokens"
        ));
    }

    #[tokio::test]
    async fn notice_maps_to_server_message() {
        let (handle, mut subs) = subscribed().await;

        handle
            .publisher()
            .publish(
                topics::Notification(NotifyName::from(residuum_bus::SYSTEM_CHANNEL)),
                NoticeEvent::new("Reflecting on the week's notes."),
            )
            .await
            .unwrap();

        let msg = subs.recv().await.unwrap();
        assert!(matches!(
            msg,
            ServerMessage::Notice { message, reload: None, reload_of: None }
                if message == "Reflecting on the week's notes."
        ));
    }

    #[tokio::test]
    async fn reload_notice_keeps_its_outcome() {
        let (handle, mut subs) = subscribed().await;

        for (outcome, of) in [
            (ReloadOutcome::Reloaded, ReloadedFiles::AgentConfig),
            (ReloadOutcome::Unchanged, ReloadedFiles::HubConfig),
            (ReloadOutcome::Failed, ReloadedFiles::Workspace),
        ] {
            handle
                .publisher()
                .publish(
                    topics::Notification(NotifyName::from(residuum_bus::SYSTEM_CHANNEL)),
                    NoticeEvent::reload(outcome, of, "configuration reloaded: models"),
                )
                .await
                .unwrap();

            let msg = subs.recv().await.unwrap();
            assert!(
                matches!(
                    &msg,
                    ServerMessage::Notice { message, reload: Some(got), reload_of: Some(got_of) }
                        if message == "configuration reloaded: models"
                            && *got == outcome
                            && *got_of == of
                ),
                "{outcome:?} of {of:?} reached the socket as {msg:?}"
            );
        }
    }

    #[tokio::test]
    async fn error_event_maps_to_server_message() {
        let (handle, mut subs) = subscribed().await;

        handle
            .publisher()
            .publish(
                topics::Notification(NotifyName::from(residuum_bus::SYSTEM_CHANNEL)),
                ErrorEvent {
                    correlation_id: "c1".into(),
                    message: "something went wrong".into(),
                    details: None,
                },
            )
            .await
            .unwrap();

        let msg = subs.recv().await.unwrap();
        assert!(matches!(
            msg,
            ServerMessage::Error { reply_to: Some(id), message, .. }
                if id == "c1" && message == "something went wrong"
        ));
    }

    #[tokio::test]
    async fn session_event_maps_to_session_frame() {
        let handle = residuum_bus::spawn_broker();
        let pub_ = handle.publisher();
        let mut subs = WsSubscribers::new(
            &handle,
            &handle,
            EndpointName::from("ws"),
            residuum_hub::file_server::FileRegistry::new("scout"),
            no_watch_set(),
            &TurnJournal::spawn(&handle).await.unwrap(),
        )
        .await
        .unwrap();

        pub_.publish(
            topics::Sessions,
            SessionEvent {
                address: residuum_bus::SessionAddress::from("spawned-x-0001"),
                run_id: "run-x".into(),
                kind: residuum_bus::SessionEventKind::Response {
                    turn_id: "run-x-t1".into(),
                    content: "found it".into(),
                },
            },
        )
        .await
        .unwrap();

        let msg = subs.recv().await.unwrap();
        assert!(
            matches!(
                msg,
                ServerMessage::SessionResponse { address, run_id, turn_id, content }
                    if address == "spawned-x-0001" && run_id == "run-x"
                        && turn_id == "run-x-t1" && content == "found it"
            ),
            "a session response must arrive as a session-tagged frame, never as a main `response`"
        );
    }

    #[tokio::test]
    async fn outbound_a2a_task_event_maps_to_session_frame() {
        let handle = residuum_bus::spawn_broker();
        let mut subs = WsSubscribers::new(
            &handle,
            &handle,
            EndpointName::from("ws"),
            residuum_hub::file_server::FileRegistry::new("scout"),
            no_watch_set(),
            &TurnJournal::spawn(&handle).await.unwrap(),
        )
        .await
        .unwrap();
        let now = chrono::Utc::now();
        handle
            .publisher()
            .publish(
                topics::Notification(NotifyName::from(residuum_bus::SYSTEM_CHANNEL)),
                OutboundA2aTaskEvent {
                    task: residuum_bus::remote_task::TrackedTask {
                        sender_address: "main".into(),
                        agent: "laptop".into(),
                        task_id: "t1".into(),
                        context_id: "c1".into(),
                        state: "working".into(),
                        last_status_text: Some("halfway".into()),
                        hop_count: 0,
                        created_at: now,
                        updated_at: now,
                        first_unreachable_at: None,
                        unreachable_notified: false,
                        notified_this_turn: false,
                        stopped_by_user: false,
                    },
                },
            )
            .await
            .unwrap();

        let msg = subs.recv().await.unwrap();
        let ServerMessage::SessionOutboundA2aTask { task } = msg else {
            panic!("expected a session_outbound_a2a_task frame, got {msg:?}");
        };
        assert_eq!(task.task_id, "t1");
        assert_eq!(task.agent, "laptop");
        assert_eq!(task.status_text.as_deref(), Some("halfway"));
        assert!(task.open);
    }

    #[tokio::test]
    async fn session_start_failed_event_maps_to_session_frame() {
        let handle = residuum_bus::spawn_broker();
        let mut subs = WsSubscribers::new(
            &handle,
            &handle,
            EndpointName::from("ws"),
            residuum_hub::file_server::FileRegistry::new("scout"),
            no_watch_set(),
            &TurnJournal::spawn(&handle).await.unwrap(),
        )
        .await
        .unwrap();
        handle
            .publisher()
            .publish(
                topics::Sessions,
                SessionStartFailedEvent {
                    address: "spawned-session-1a2b".into(),
                    error: "The session spawned-session-1a2b couldn't start.".into(),
                    error_details: Some("failed to build provider chain: no key".into()),
                },
            )
            .await
            .unwrap();

        let msg = subs.recv().await.unwrap();
        let ServerMessage::SessionStartFailed {
            address,
            error,
            error_details,
        } = msg
        else {
            panic!("expected a session_start_failed frame, got {msg:?}");
        };
        assert_eq!(address, "spawned-session-1a2b");
        assert_eq!(error, "The session spawned-session-1a2b couldn't start.");
        assert_eq!(
            error_details.as_deref(),
            Some("failed to build provider chain: no key")
        );
    }

    fn no_watch_set() -> tokio::sync::watch::Receiver<WatchSet> {
        tokio::sync::watch::channel(WatchSet::default()).1
    }

    fn modified(path: &str) -> WorkspaceChange {
        WorkspaceChange {
            path: path.to_string(),
            kind: WorkspaceChangeKind::Modified,
        }
    }

    fn batch(paths: &[&str]) -> WorkspaceEvent {
        WorkspaceEvent::Changed(paths.iter().map(|p| modified(p)).collect())
    }

    #[tokio::test]
    async fn workspace_batches_are_filtered_by_the_live_watch_set() {
        let handle = residuum_bus::spawn_broker();
        let (watch_tx, watch_rx) = tokio::sync::watch::channel(WatchSet::default());
        let mut subs = WsSubscribers::new(
            &handle,
            &handle,
            EndpointName::from("ws"),
            residuum_hub::file_server::FileRegistry::new("scout"),
            watch_rx,
            &TurnJournal::spawn(&handle).await.unwrap(),
        )
        .await
        .unwrap();
        // The connection starts watching after it subscribed; batches from
        // then on are filtered by the set it sent.
        watch_tx.send_replace(WatchSet::parse(vec!["wiki".into()]).unwrap());
        handle
            .publisher()
            .publish(topics::Workspace, batch(&["notes/x.md", "wiki/a.md"]))
            .await
            .unwrap();
        let msg = subs.recv().await.unwrap();
        assert!(
            matches!(&msg, ServerMessage::WorkspaceChanged { changes } if changes == &[modified("wiki/a.md")]),
            "{msg:?}"
        );
    }
}
