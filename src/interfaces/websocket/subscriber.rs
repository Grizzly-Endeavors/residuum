//! WebSocket bus subscriber — translates typed bus events to `ServerMessage` frames.

use crate::bus::{
    EndpointName, ErrorEvent, InlineOutputEvent, IntermediateEvent, NoticeEvent, NotifyName,
    PostTurnActivityEvent, PostTurnActivityKind, ResponseEvent, SessionEvent, Subscriber,
    ToolActivityEvent, TurnLifecycleEvent, TurnUsageEvent, WorkbenchEvent, WorkspaceEvent, topics,
};
use crate::gateway::file_server::FileRegistry;
use crate::gateway::protocol::ServerMessage;
use crate::workspace::watch::{
    LIVE_UPDATES_OFF_MESSAGE, WatchSet, WatchedChanges, WorkspaceResyncReason,
};

/// The frame for a main-agent tool call or result.
fn tool_activity_frame(activity: ToolActivityEvent) -> ServerMessage {
    match activity {
        ToolActivityEvent::Call(tc) => ServerMessage::ToolCall {
            id: tc.tool_call_id,
            name: tc.name,
            arguments: tc.arguments,
        },
        ToolActivityEvent::Result(tr) => ServerMessage::ToolResult {
            tool_call_id: tr.tool_call_id,
            name: tr.name,
            output: tr.output,
            is_error: tr.is_error,
        },
    }
}

/// The frame for a main-agent turn lifecycle transition.
fn turn_lifecycle_frame(event: TurnLifecycleEvent) -> ServerMessage {
    match event {
        TurnLifecycleEvent::Started { correlation_id } => ServerMessage::TurnStarted {
            reply_to: correlation_id,
        },
        TurnLifecycleEvent::Ended { correlation_id } => ServerMessage::TurnEnded {
            reply_to: correlation_id,
        },
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
        session_totals: usage.session_totals,
    }
}

/// The frame a workspace change-feed event becomes for a connection watching
/// `watch_set`, if any. A connection watching nothing gets nothing.
fn workspace_frame(watch_set: &WatchSet, event: WorkspaceEvent) -> Option<ServerMessage> {
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

/// Convert a `ResponseEvent` into the appropriate `ServerMessage`.
///
/// If the response carries a file attachment, registers it with the file
/// registry and returns a `FileAttachment` frame; otherwise returns a plain
/// `Response` frame. Extracted from `WsSubscribers::recv` to keep the select
/// loop within clippy's `too_many_lines` budget.
async fn response_to_server_message(registry: &FileRegistry, resp: ResponseEvent) -> ServerMessage {
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
            content: resp.content,
        }
    }
}

/// Typed subscribers for a single WebSocket connection.
pub struct WsSubscribers {
    pub response: Subscriber<ResponseEvent>,
    pub tool_activity: Subscriber<ToolActivityEvent>,
    pub turn_lifecycle: Subscriber<TurnLifecycleEvent>,
    pub turn_usage: Subscriber<TurnUsageEvent>,
    /// Background post-turn cycle start/finish, for the quiet activity
    /// indicator — see `crate::gateway::post_turn`.
    pub post_turn_activity: Subscriber<PostTurnActivityEvent>,
    pub intermediate: Subscriber<IntermediateEvent>,
    pub notice: Subscriber<NoticeEvent>,
    pub inline_output: Subscriber<InlineOutputEvent>,
    pub error: Subscriber<ErrorEvent>,
    /// Agent session lifecycle and turn events, forwarded as the
    /// `session_*` frames. Main-agent frames never come from here.
    pub session: Subscriber<SessionEvent>,
    /// Workbench artifact file changes, so an open artifact view reloads live.
    pub workbench: Subscriber<WorkbenchEvent>,
    /// The workspace change feed, filtered by `watch_set`.
    pub workspace: Subscriber<WorkspaceEvent>,
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
        ep: EndpointName,
        file_registry: crate::gateway::file_server::FileRegistry,
        watch_set: tokio::sync::watch::Receiver<WatchSet>,
    ) -> Result<Self, crate::bus::BusError> {
        let system_topic = || topics::Notification(NotifyName::from(crate::bus::SYSTEM_CHANNEL));
        Ok(Self {
            response: bus_handle.subscribe(topics::Endpoint(ep.clone())).await?,
            tool_activity: bus_handle.subscribe(topics::Endpoint(ep.clone())).await?,
            turn_lifecycle: bus_handle.subscribe(topics::Endpoint(ep.clone())).await?,
            turn_usage: bus_handle.subscribe(topics::Endpoint(ep.clone())).await?,
            intermediate: bus_handle.subscribe(topics::Endpoint(ep)).await?,
            post_turn_activity: bus_handle.subscribe(system_topic()).await?,
            notice: bus_handle.subscribe(system_topic()).await?,
            inline_output: bus_handle.subscribe(system_topic()).await?,
            error: bus_handle.subscribe(system_topic()).await?,
            session: bus_handle.subscribe(topics::Sessions).await?,
            workbench: bus_handle.subscribe(topics::Workbench).await?,
            workspace: bus_handle.subscribe(topics::Workspace).await?,
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
                event = self.response.recv() => {
                    match event {
                        Ok(Some(resp)) => Some(
                            response_to_server_message(&self.file_registry, resp).await,
                        ),
                        _ => return None,
                    }
                }
                event = self.tool_activity.recv() => match event {
                    Ok(Some(activity)) => Some(tool_activity_frame(activity)),
                    _ => return None,
                },
                event = self.turn_lifecycle.recv() => match event {
                    Ok(Some(lifecycle)) => Some(turn_lifecycle_frame(lifecycle)),
                    _ => return None,
                },
                event = self.turn_usage.recv() => {
                    match event {
                        Ok(Some(usage)) => Some(turn_usage_frame(usage)),
                        _ => return None,
                    }
                }
                event = self.post_turn_activity.recv() => match event {
                    Ok(Some(activity)) => Some(post_turn_activity_frame(activity)),
                    _ => return None,
                },
                event = self.intermediate.recv() => {
                    match event {
                        Ok(Some(im)) => Some(ServerMessage::BroadcastResponse {
                            content: im.content,
                        }),
                        _ => return None,
                    }
                }
                event = self.notice.recv() => {
                    match event {
                        Ok(Some(NoticeEvent { message })) => {
                            Some(ServerMessage::Notice { message })
                        }
                        _ => return None,
                    }
                }
                event = self.inline_output.recv() => {
                    match event {
                        Ok(Some(InlineOutputEvent { message })) => {
                            Some(ServerMessage::InlineOutput { message })
                        }
                        _ => return None,
                    }
                }
                event = self.session.recv() => {
                    match event {
                        Ok(Some(session_event)) => Some(
                            crate::gateway::sessions::session_event_to_server_message(session_event),
                        ),
                        _ => return None,
                    }
                }
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
    use crate::bus::{
        IntermediateEvent, NotifyName, ResponseEvent, ToolCallEvent, ToolResultEvent,
    };
    use crate::workspace::watch::{WorkspaceChange, WorkspaceChangeKind};

    fn ts() -> chrono::NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 3, 13)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
    }

    #[tokio::test]
    async fn response_maps_to_server_message() {
        let handle = crate::bus::spawn_broker();
        let pub_ = handle.publisher();
        let ep = EndpointName::from("ws");
        let mut subs = WsSubscribers::new(
            &handle,
            ep.clone(),
            crate::gateway::file_server::FileRegistry::new(),
            no_watch_set(),
        )
        .await
        .unwrap();

        pub_.publish(
            topics::Endpoint(ep),
            ResponseEvent {
                correlation_id: "c1".into(),
                content: "hello".into(),
                timestamp: ts(),
                attachment: None,
                conversation: None,
            },
        )
        .await
        .unwrap();

        let msg = subs.recv().await.unwrap();
        assert!(matches!(
            msg,
            ServerMessage::Response { reply_to, content }
                if reply_to == "c1" && content == "hello"
        ));
    }

    #[tokio::test]
    async fn tool_call_maps_to_server_message() {
        let handle = crate::bus::spawn_broker();
        let pub_ = handle.publisher();
        let ep = EndpointName::from("ws");
        let mut subs = WsSubscribers::new(
            &handle,
            ep.clone(),
            crate::gateway::file_server::FileRegistry::new(),
            no_watch_set(),
        )
        .await
        .unwrap();

        pub_.publish(
            topics::Endpoint(ep),
            ToolActivityEvent::Call(ToolCallEvent {
                correlation_id: "c1".into(),
                tool_call_id: "tc1".into(),
                name: "search".into(),
                arguments: serde_json::json!({"q": "test"}),
            }),
        )
        .await
        .unwrap();

        let msg = subs.recv().await.unwrap();
        assert!(matches!(
            msg,
            ServerMessage::ToolCall { id, name, .. }
                if id == "tc1" && name == "search"
        ));
    }

    #[tokio::test]
    async fn tool_result_maps_to_server_message() {
        let handle = crate::bus::spawn_broker();
        let pub_ = handle.publisher();
        let ep = EndpointName::from("ws");
        let mut subs = WsSubscribers::new(
            &handle,
            ep.clone(),
            crate::gateway::file_server::FileRegistry::new(),
            no_watch_set(),
        )
        .await
        .unwrap();

        pub_.publish(
            topics::Endpoint(ep),
            ToolActivityEvent::Result(ToolResultEvent {
                correlation_id: "c1".into(),
                tool_call_id: "tc1".into(),
                name: "search".into(),
                output: "found it".into(),
                is_error: false,
            }),
        )
        .await
        .unwrap();

        let msg = subs.recv().await.unwrap();
        assert!(matches!(
            msg,
            ServerMessage::ToolResult { tool_call_id, name, output, is_error }
                if tool_call_id == "tc1" && name == "search" && output == "found it" && !is_error
        ));
    }

    #[tokio::test]
    async fn intermediate_maps_to_broadcast_response() {
        let handle = crate::bus::spawn_broker();
        let pub_ = handle.publisher();
        let ep = EndpointName::from("ws");
        let mut subs = WsSubscribers::new(
            &handle,
            ep.clone(),
            crate::gateway::file_server::FileRegistry::new(),
            no_watch_set(),
        )
        .await
        .unwrap();

        pub_.publish(
            topics::Endpoint(ep),
            IntermediateEvent {
                correlation_id: "c1".into(),
                content: "thinking...".into(),
            },
        )
        .await
        .unwrap();

        let msg = subs.recv().await.unwrap();
        assert!(matches!(
            msg,
            ServerMessage::BroadcastResponse { content }
                if content == "thinking..."
        ));
    }

    #[tokio::test]
    async fn inline_output_maps_to_server_message() {
        let handle = crate::bus::spawn_broker();
        let pub_ = handle.publisher();
        let ep = EndpointName::from("ws");
        let mut subs = WsSubscribers::new(
            &handle,
            ep,
            crate::gateway::file_server::FileRegistry::new(),
            no_watch_set(),
        )
        .await
        .unwrap();

        pub_.publish(
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
        let handle = crate::bus::spawn_broker();
        let pub_ = handle.publisher();
        let ep = EndpointName::from("ws");
        let mut subs = WsSubscribers::new(
            &handle,
            ep,
            crate::gateway::file_server::FileRegistry::new(),
            no_watch_set(),
        )
        .await
        .unwrap();

        pub_.publish(
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
    async fn turn_started_maps_to_server_message() {
        let handle = crate::bus::spawn_broker();
        let pub_ = handle.publisher();
        let ep = EndpointName::from("ws");
        let mut subs = WsSubscribers::new(
            &handle,
            ep.clone(),
            crate::gateway::file_server::FileRegistry::new(),
            no_watch_set(),
        )
        .await
        .unwrap();

        pub_.publish(
            topics::Endpoint(ep),
            TurnLifecycleEvent::Started {
                correlation_id: "c1".into(),
            },
        )
        .await
        .unwrap();

        let msg = subs.recv().await.unwrap();
        assert!(matches!(
            msg,
            ServerMessage::TurnStarted { reply_to }
                if reply_to == "c1"
        ));
    }

    #[tokio::test]
    async fn turn_ended_maps_to_server_message() {
        let handle = crate::bus::spawn_broker();
        let pub_ = handle.publisher();
        let ep = EndpointName::from("ws");
        let mut subs = WsSubscribers::new(
            &handle,
            ep.clone(),
            crate::gateway::file_server::FileRegistry::new(),
            no_watch_set(),
        )
        .await
        .unwrap();

        pub_.publish(
            topics::Endpoint(ep),
            TurnLifecycleEvent::Ended {
                correlation_id: "c1".into(),
            },
        )
        .await
        .unwrap();

        let msg = subs.recv().await.unwrap();
        assert!(
            matches!(msg, ServerMessage::TurnEnded { reply_to } if reply_to == "c1"),
            "TurnEnded should map to ServerMessage::TurnEnded"
        );
    }

    #[tokio::test]
    async fn turn_usage_maps_to_server_message() {
        let handle = crate::bus::spawn_broker();
        let pub_ = handle.publisher();
        let ep = EndpointName::from("ws");
        let mut subs = WsSubscribers::new(
            &handle,
            ep.clone(),
            crate::gateway::file_server::FileRegistry::new(),
            no_watch_set(),
        )
        .await
        .unwrap();

        let mut totals = crate::agent::usage::SessionUsageTotals::default();
        totals.accumulate(Some(crate::inference::Usage {
            input_tokens: 100,
            output_tokens: 20,
            cache_creation_tokens: None,
            cache_read_tokens: None,
        }));

        pub_.publish(
            topics::Endpoint(ep),
            crate::bus::TurnUsageEvent {
                correlation_id: "c1".into(),
                output_tokens: 20,
                has_usage: true,
                session_totals: Some(totals),
            },
        )
        .await
        .unwrap();

        let msg = subs.recv().await.unwrap();
        assert!(
            matches!(
                &msg,
                ServerMessage::TurnUsage { reply_to, output_tokens: 20, has_usage: true, session_totals: Some(t) }
                    if reply_to == "c1" && *t == totals
            ),
            "TurnUsageEvent should map to ServerMessage::TurnUsage: {msg:?}"
        );
    }

    #[tokio::test]
    async fn error_event_maps_to_server_message() {
        let handle = crate::bus::spawn_broker();
        let pub_ = handle.publisher();
        let ep = EndpointName::from("ws");
        let mut subs = WsSubscribers::new(
            &handle,
            ep,
            crate::gateway::file_server::FileRegistry::new(),
            no_watch_set(),
        )
        .await
        .unwrap();

        pub_.publish(
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
            EndpointName::from("ws"),
            crate::gateway::file_server::FileRegistry::new(),
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
            EndpointName::from("ws"),
            crate::gateway::file_server::FileRegistry::new(),
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
