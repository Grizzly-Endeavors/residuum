//! WebSocket bus subscriber — translates typed bus events to `ServerMessage` frames.

use crate::bus::{
    EndpointName, ErrorEvent, InlineOutputEvent, MainConversationEvent, NoticeEvent, NotifyName,
    OutboundA2aTaskEvent, PostTurnActivityEvent, PostTurnActivityKind, ResponseEvent, SessionEvent,
    Subscriber, TurnUsageEvent, WorkbenchEvent, WorkspaceEvent, topics,
};
use crate::gateway::file_server::FileRegistry;
use crate::gateway::protocol::ServerMessage;
use crate::workspace::watch::{
    LIVE_UPDATES_OFF_MESSAGE, WatchSet, WatchedChanges, WorkspaceResyncReason,
};

/// The frame for one event of the main agent's conversation.
fn main_conversation_frame(event: MainConversationEvent) -> ServerMessage {
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
            endpoint,
        } => ServerMessage::UserMessage {
            id,
            turn_id,
            content,
            images,
            sender,
            endpoint,
        },
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

/// The frame for a background post-turn cycle (see
/// `crate::gateway::post_turn`) starting or finishing.
fn post_turn_activity_frame(event: PostTurnActivityEvent) -> ServerMessage {
    ServerMessage::PostTurnActivity {
        kind: match event.kind {
            PostTurnActivityKind::Memory => crate::gateway::protocol::PostTurnActivityKind::Memory,
            PostTurnActivityKind::Subconscious => {
                crate::gateway::protocol::PostTurnActivityKind::Subconscious
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

/// The frame a workspace change-feed event becomes for a connection watching
/// `watch_set`, if any. A connection watching nothing gets nothing.
pub(crate) fn workspace_frame(
    watch_set: &WatchSet,
    event: WorkspaceEvent,
) -> Option<ServerMessage> {
    if watch_set.is_empty() {
        return None;
    }
    match event {
        WorkspaceEvent::Changed(changes) => match watch_set.filter(&changes) {
            WatchedChanges::None => None,
            WatchedChanges::Changes(changes) => Some(ServerMessage::WorkspaceChanged { changes }),
            WatchedChanges::TooMany => Some(ServerMessage::WorkspaceResync {
                reason: WorkspaceResyncReason::Overflow,
            }),
        },
        WorkspaceEvent::Resync(reason) => Some(ServerMessage::WorkspaceResync { reason }),
        WorkspaceEvent::Unavailable => Some(ServerMessage::WorkspaceWatchUnavailable {
            message: LIVE_UPDATES_OFF_MESSAGE.to_string(),
        }),
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
    /// Every main-agent turn, whatever endpoint started it, in order: the
    /// source of all the main conversation's frames — turn lifecycle, user
    /// messages, tool activity, thinking, streamed text, intermediate text,
    /// replies and usage.
    pub main: Subscriber<MainConversationEvent>,
    /// Messages posted to this endpoint with `send_message`, files included.
    /// Nothing else is published to the web UI's endpoint: a turn's reply
    /// arrives through `main`.
    pub response: Subscriber<ResponseEvent>,
    /// The endpoint `response` listens on, named in the frames it yields.
    endpoint: EndpointName,
    /// Background post-turn cycle start/finish, for the quiet activity
    /// indicator — see `crate::gateway::post_turn`.
    pub post_turn_activity: Subscriber<PostTurnActivityEvent>,
    pub notice: Subscriber<NoticeEvent>,
    pub inline_output: Subscriber<InlineOutputEvent>,
    pub error: Subscriber<ErrorEvent>,
    /// Agent session lifecycle and turn events, forwarded as the
    /// `session_*` frames. Main-agent frames never come from here.
    pub session: Subscriber<SessionEvent>,
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
    pub file_registry: crate::gateway::file_server::FileRegistry,
}

impl WsSubscribers {
    /// Create all typed subscribers for a WebSocket connection.
    ///
    /// # Errors
    ///
    /// Returns `BusError` if any subscription fails.
    pub async fn new(
        bus_handle: &crate::bus::BusHandle,
        team_bus: &crate::bus::BusHandle,
        ep: EndpointName,
        file_registry: crate::gateway::file_server::FileRegistry,
        watch_set: tokio::sync::watch::Receiver<WatchSet>,
    ) -> Result<Self, crate::bus::BusError> {
        let system_topic = || topics::Notification(NotifyName::from(crate::bus::SYSTEM_CHANNEL));
        Ok(Self {
            main: bus_handle.subscribe(topics::MainConversation).await?,
            response: bus_handle.subscribe(topics::Endpoint(ep.clone())).await?,
            endpoint: ep,
            post_turn_activity: bus_handle.subscribe(system_topic()).await?,
            notice: bus_handle.subscribe(system_topic()).await?,
            inline_output: bus_handle.subscribe(system_topic()).await?,
            error: bus_handle.subscribe(system_topic()).await?,
            session: bus_handle.subscribe(topics::Sessions).await?,
            outbound_a2a: bus_handle.subscribe(system_topic()).await?,
            workbench: bus_handle.subscribe(topics::Workbench).await?,
            workspace: bus_handle.subscribe(topics::Workspace).await?,
            team_workspace: team_bus.subscribe(topics::Workspace).await?,
            watch_set,
            file_registry,
        })
    }

    /// Receive the next server message from any subscribed topic.
    ///
    /// Returns `None` when all subscribers have closed.
    pub async fn recv(&mut self) -> Option<ServerMessage> {
        loop {
            let msg = tokio::select! {
                event = self.main.recv() => match event {
                    Ok(Some(main_event)) => Some(main_conversation_frame(main_event)),
                    _ => return None,
                },
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
                    Ok(Some(NoticeEvent { message })) => Some(ServerMessage::Notice { message }),
                    _ => return None,
                },
                event = self.inline_output.recv() => match event {
                    Ok(Some(InlineOutputEvent { message })) => {
                        Some(ServerMessage::InlineOutput { message })
                    }
                    _ => return None,
                },
                event = self.session.recv() => match event {
                    Ok(Some(session_event)) => Some(
                        crate::gateway::sessions::session_event_to_server_message(session_event),
                    ),
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
    use crate::bus::{ToolCallEvent, ToolResultEvent};
    use crate::workspace::watch::{WorkspaceChange, WorkspaceChangeKind};

    fn ts() -> chrono::NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 3, 13)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
    }

    /// A broker with a `WsSubscribers` on the `ws` endpoint.
    async fn subscribed() -> (crate::bus::BusHandle, WsSubscribers) {
        let handle = crate::bus::spawn_broker();
        let subs = WsSubscribers::new(
            &handle,
            &handle,
            EndpointName::from("ws"),
            crate::gateway::file_server::FileRegistry::new("scout"),
            no_watch_set(),
        )
        .await
        .unwrap();
        (handle, subs)
    }

    async fn publish_main(handle: &crate::bus::BusHandle, event: MainConversationEvent) {
        handle
            .publisher()
            .publish(topics::MainConversation, event)
            .await
            .unwrap();
    }

    fn origin(endpoint: &str) -> crate::bus::TurnOrigin {
        crate::bus::TurnOrigin {
            endpoint: endpoint.into(),
            sender: None,
            visibility: crate::memory::types::Visibility::User,
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

        let mut totals = crate::agent::usage::SessionUsageTotals::default();
        totals.accumulate(Some(crate::inference::Usage {
            input_tokens: 100,
            output_tokens: 20,
            cache_creation_tokens: None,
            cache_read_tokens: None,
        }));

        publish_main(
            &handle,
            MainConversationEvent::TurnUsage(crate::bus::TurnUsageEvent {
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
                topics::Notification(NotifyName::from(crate::bus::SYSTEM_CHANNEL)),
                crate::bus::InlineOutputEvent {
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
                topics::Notification(NotifyName::from(crate::bus::SYSTEM_CHANNEL)),
                NoticeEvent {
                    message: "reloading".into(),
                },
            )
            .await
            .unwrap();

        let msg = subs.recv().await.unwrap();
        assert!(matches!(
            msg,
            ServerMessage::Notice { message }
                if message == "reloading"
        ));
    }

    #[tokio::test]
    async fn error_event_maps_to_server_message() {
        let (handle, mut subs) = subscribed().await;

        handle
            .publisher()
            .publish(
                topics::Notification(NotifyName::from(crate::bus::SYSTEM_CHANNEL)),
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
        let handle = crate::bus::spawn_broker();
        let pub_ = handle.publisher();
        let mut subs = WsSubscribers::new(
            &handle,
            &handle,
            EndpointName::from("ws"),
            crate::gateway::file_server::FileRegistry::new("scout"),
            no_watch_set(),
        )
        .await
        .unwrap();

        pub_.publish(
            topics::Sessions,
            SessionEvent {
                address: crate::bus::SessionAddress::from("spawned-x-0001"),
                run_id: "run-x".into(),
                kind: crate::bus::SessionEventKind::Response {
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
        let handle = crate::bus::spawn_broker();
        let mut subs = WsSubscribers::new(
            &handle,
            &handle,
            EndpointName::from("ws"),
            crate::gateway::file_server::FileRegistry::new("scout"),
            no_watch_set(),
        )
        .await
        .unwrap();
        let now = chrono::Utc::now();
        handle
            .publisher()
            .publish(
                topics::Notification(NotifyName::from(crate::bus::SYSTEM_CHANNEL)),
                OutboundA2aTaskEvent {
                    task: crate::a2a::TrackedTask {
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

    fn no_watch_set() -> tokio::sync::watch::Receiver<WatchSet> {
        tokio::sync::watch::channel(WatchSet::default()).1
    }

    fn watching(prefixes: &[&str]) -> tokio::sync::watch::Receiver<WatchSet> {
        let set = WatchSet::parse(prefixes.iter().map(ToString::to_string).collect()).unwrap();
        tokio::sync::watch::channel(set).1
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

    #[test]
    fn a_connection_receives_only_changes_under_its_prefixes() {
        let frame = workspace_frame(
            &watching(&["wiki"]).borrow(),
            batch(&["wiki/a.md", "wikipedia/b.md", "notes/c.md"]),
        );
        assert!(
            matches!(&frame, Some(ServerMessage::WorkspaceChanged { changes }) if changes == &[modified("wiki/a.md")]),
            "{frame:?}"
        );
        assert!(
            workspace_frame(&watching(&["notes/other"]).borrow(), batch(&["wiki/a.md"])).is_none()
        );
    }

    #[test]
    fn a_connection_watching_nothing_receives_nothing() {
        let set = no_watch_set();
        assert!(workspace_frame(&set.borrow(), batch(&["wiki/a.md"])).is_none());
        assert!(
            workspace_frame(
                &set.borrow(),
                WorkspaceEvent::Resync(WorkspaceResyncReason::WatcherRestarted)
            )
            .is_none()
        );
        assert!(workspace_frame(&set.borrow(), WorkspaceEvent::Unavailable).is_none());
    }

    #[test]
    fn too_many_matching_changes_become_an_overflow_resync() {
        let paths: Vec<String> = (0..=crate::workspace::watch::MAX_CHANGES_PER_FRAME)
            .map(|i| format!("wiki/{i}.md"))
            .collect();
        let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
        let frame = workspace_frame(&watching(&["wiki"]).borrow(), batch(&paths));
        assert!(matches!(
            frame,
            Some(ServerMessage::WorkspaceResync {
                reason: WorkspaceResyncReason::Overflow
            })
        ));
    }

    #[test]
    fn resyncs_and_outages_reach_every_watching_connection() {
        let set = watching(&["wiki"]);
        assert!(matches!(
            workspace_frame(
                &set.borrow(),
                WorkspaceEvent::Resync(WorkspaceResyncReason::Overflow)
            ),
            Some(ServerMessage::WorkspaceResync {
                reason: WorkspaceResyncReason::Overflow
            })
        ));
        assert!(matches!(
            workspace_frame(&set.borrow(), WorkspaceEvent::Unavailable),
            Some(ServerMessage::WorkspaceWatchUnavailable { .. })
        ));
    }

    #[tokio::test]
    async fn workspace_batches_are_filtered_by_the_live_watch_set() {
        let handle = crate::bus::spawn_broker();
        let (watch_tx, watch_rx) = tokio::sync::watch::channel(WatchSet::default());
        let mut subs = WsSubscribers::new(
            &handle,
            &handle,
            EndpointName::from("ws"),
            crate::gateway::file_server::FileRegistry::new("scout"),
            watch_rx,
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
