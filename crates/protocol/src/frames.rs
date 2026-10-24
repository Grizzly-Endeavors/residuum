//! The wire frames sent to clients, built from bus session events, registry
//! entries and the workspace change feed.

use residuum_bus::session::SessionInfo;
use residuum_bus::workspace_change::WorkspaceResyncReason;
use residuum_bus::{
    AgentResultStatus, SessionEvent, SessionEventKind, SessionStartFailedEvent, WorkspaceEvent,
};
use residuum_workspace::watch::{LIVE_UPDATES_OFF_MESSAGE, WatchSet, WatchedChanges};

use super::{ServerMessage, SessionRunStatus, SessionSummary};

/// Summarize a live run from its registry entry.
#[must_use]
pub fn summary_from_live(info: &SessionInfo) -> SessionSummary {
    SessionSummary {
        address: info.address.to_string(),
        run_id: info.run_id.clone(),
        category: info.category,
        source_label: info.source_label.clone(),
        state: info.state,
        spawner: info.spawner.as_ref().map(ToString::to_string),
        depth: info.depth,
        purpose: info.purpose.clone(),
        started_at: info.started_at,
        completed_at: None,
        episode_id: None,
        interrupted: false,
        usage: info.usage,
        // A live run has no outcome yet; `session_completed` carries it when
        // the run finishes (see `session_event_to_server_message` below).
        outcome: None,
        error: None,
        error_details: None,
        overlap: info.overlap.clone(),
        model: Some(info.model.to_string()),
        resolved_model: info.resolved_model.clone(),
    }
}

/// The frame for an owner-started session that couldn't be forked; `None`
/// for any other session's failure (a pulse fire's), because the web UI keeps
/// a failure per address until a run starts there, and a pulse fire's
/// address never gets one.
#[must_use]
pub fn session_start_failed_frame(event: SessionStartFailedEvent) -> Option<ServerMessage> {
    let SessionStartFailedEvent {
        address,
        error,
        error_details,
        owner_start,
    } = event;
    owner_start.then(|| ServerMessage::SessionStartFailed {
        address: address.to_string(),
        error,
        error_details,
    })
}

/// Translate a bus session event into the WebSocket frame clients receive.
#[must_use]
pub fn session_event_to_server_message(event: SessionEvent) -> ServerMessage {
    let SessionEvent {
        address,
        run_id,
        kind,
    } = event;
    let address = address.to_string();
    match kind {
        SessionEventKind::Started(info) => ServerMessage::SessionStarted {
            session: summary_from_live(&info),
        },
        SessionEventKind::StateChanged(state) => ServerMessage::SessionStateChanged {
            address,
            run_id,
            state,
        },
        SessionEventKind::Completed { status, episode_id } => {
            completed_frame(address, run_id, status, episode_id)
        }
        SessionEventKind::TurnStarted { turn_id } => ServerMessage::SessionTurnStarted {
            address,
            run_id,
            turn_id,
        },
        SessionEventKind::TurnEnded { turn_id } => ServerMessage::SessionTurnEnded {
            address,
            run_id,
            turn_id,
        },
        SessionEventKind::MessageRead {
            id,
            turn_id,
            content,
            from,
            from_category,
        } => ServerMessage::SessionUserMessage {
            address,
            run_id,
            id,
            turn_id,
            content,
            from,
            from_category,
        },
        SessionEventKind::ToolCall(call) => ServerMessage::SessionToolCall {
            address,
            run_id,
            id: call.tool_call_id,
            name: call.name,
            arguments: call.arguments,
            server: call.server,
        },
        SessionEventKind::ToolResult(result) => ServerMessage::SessionToolResult {
            address,
            run_id,
            tool_call_id: result.tool_call_id,
            name: result.name,
            output: result.output,
            is_error: result.is_error,
            auto_mode: result.auto_mode,
        },
        SessionEventKind::Intermediate { content } => ServerMessage::SessionBroadcastResponse {
            address,
            run_id,
            content,
        },
        SessionEventKind::TurnUsage {
            output_tokens,
            has_usage,
            tool_calls,
            session_totals,
        } => ServerMessage::SessionTurnUsage {
            address,
            run_id,
            output_tokens,
            has_usage,
            tool_calls,
            session_totals,
        },
        SessionEventKind::Response { turn_id, content } => ServerMessage::SessionResponse {
            address,
            run_id,
            turn_id,
            content,
        },
        SessionEventKind::Error { message, details } => ServerMessage::SessionError {
            address,
            run_id,
            message,
            details,
        },
    }
}

/// The `SessionCompleted` frame for a run that finished with `status`.
fn completed_frame(
    address: String,
    run_id: String,
    status: AgentResultStatus,
    episode_id: Option<String>,
) -> ServerMessage {
    let (status, error, error_details) = match status {
        AgentResultStatus::Completed => (SessionRunStatus::Completed, None, None),
        AgentResultStatus::Cancelled => (SessionRunStatus::Cancelled, None, None),
        AgentResultStatus::Failed { error, details } => {
            (SessionRunStatus::Failed, Some(error), details)
        }
    };
    ServerMessage::SessionCompleted {
        address,
        run_id,
        status,
        error,
        error_details,
        episode_id,
    }
}

/// The frame a workspace change-feed event becomes for a connection watching
/// `watch_set`, if any. A connection watching nothing gets nothing.
#[must_use]
pub fn workspace_frame(watch_set: &WatchSet, event: WorkspaceEvent) -> Option<ServerMessage> {
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

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;
    use residuum_bus::session::{MAIN_ADDRESS, SessionCategory, SessionState};
    use residuum_bus::workspace_change::{WorkspaceChange, WorkspaceChangeKind};
    use residuum_bus::{EventTrigger, SessionAddress, ToolCallEvent};

    fn live_info(address: &str, state: SessionState) -> SessionInfo {
        SessionInfo {
            address: SessionAddress::from(address),
            run_id: format!("run-{address}"),
            category: SessionCategory::Spawned,
            trigger: EventTrigger::Agent,
            source_label: "agent:researcher".to_string(),
            state,
            spawner: Some(SessionAddress::from(MAIN_ADDRESS)),
            depth: 1,
            purpose: "research".to_string(),
            agent_skill: None,
            model: residuum_config::ModelSelector::MEDIUM,
            resolved_model: None,
            conversation_target: None,
            started_at: Utc::now(),
            usage: residuum_bus::usage::SessionUsageTotals::default(),
            overlap: None,
        }
    }

    #[test]
    fn completed_event_maps_failure_reason_into_its_own_field() {
        let msg = session_event_to_server_message(SessionEvent {
            address: SessionAddress::from("spawned-d-0001"),
            run_id: "run-d".to_string(),
            kind: SessionEventKind::Completed {
                status: AgentResultStatus::Failed {
                    error: "model down".to_string(),
                    details: None,
                },
                episode_id: Some("ep-007".to_string()),
            },
        });
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "type": "session_completed",
                "address": "spawned-d-0001",
                "run_id": "run-d",
                "status": "failed",
                "error": "model down",
                "episode_id": "ep-007",
            })
        );
    }

    #[test]
    fn message_read_event_becomes_a_session_user_message_frame() {
        let message = residuum_bus::AgentMessageEvent::new(
            SessionAddress::from(residuum_bus::session::OWNER_ADDRESS),
            "owner",
            "check the logs too",
            0,
        )
        .with_id("m-42");
        let msg = session_event_to_server_message(SessionEvent {
            address: SessionAddress::from("spawned-d-0003"),
            run_id: "run-d3".to_string(),
            kind: message.read_event("run-d3-t2"),
        });
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "type": "session_user_message",
                "address": "spawned-d-0003",
                "run_id": "run-d3",
                "id": "m-42",
                "turn_id": "run-d3-t2",
                "content": "check the logs too",
                "from": "owner",
                "from_category": "owner",
            }),
            "the frame carries the message as sent, without the header the session sees"
        );
    }

    #[test]
    fn completed_event_carries_the_full_cause_chain_when_there_is_one() {
        let msg = session_event_to_server_message(SessionEvent {
            address: SessionAddress::from("spawned-d-0002"),
            run_id: "run-d2".to_string(),
            kind: SessionEventKind::Completed {
                status: AgentResultStatus::Failed {
                    error: "model down".to_string(),
                    details: Some("connect timeout after 30s".to_string()),
                },
                episode_id: None,
            },
        });
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "type": "session_completed",
                "address": "spawned-d-0002",
                "run_id": "run-d2",
                "status": "failed",
                "error": "model down",
                "error_details": "connect timeout after 30s",
                "episode_id": null,
            }),
            "the full cause chain must reach the client alongside the plain message"
        );
    }

    #[test]
    fn started_event_carries_the_full_summary() {
        let info = live_info("spawned-e-0001", SessionState::Forking);
        let msg = session_event_to_server_message(SessionEvent {
            address: info.address.clone(),
            run_id: info.run_id.clone(),
            kind: SessionEventKind::Started(Box::new(info.clone())),
        });
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "type": "session_started",
                "session": {
                    "address": "spawned-e-0001",
                    "run_id": info.run_id,
                    "category": "spawned",
                    "source_label": "agent:researcher",
                    "state": "forking",
                    "spawner": "main",
                    "depth": 1,
                    "purpose": "research",
                    "started_at": serde_json::to_value(info.started_at).unwrap(),
                    "completed_at": null,
                    "episode_id": null,
                    "interrupted": false,
                    "usage": {
                        "input_tokens": 0,
                        "output_tokens": 0,
                        "context_tokens": null,
                        "tool_calls": 0,
                    },
                    "outcome": null,
                    "error": null,
                    "error_details": null,
                    "overlap": null,
                    "model": "medium",
                    "resolved_model": null,
                },
            })
        );
    }

    #[test]
    fn tool_call_event_is_tagged_with_address_and_run() {
        let msg = session_event_to_server_message(SessionEvent {
            address: SessionAddress::from("spawned-f-0001"),
            run_id: "run-f".to_string(),
            kind: SessionEventKind::ToolCall(ToolCallEvent {
                correlation_id: String::new(),
                tool_call_id: "tc-1".to_string(),
                name: "memory_search".to_string(),
                arguments: serde_json::json!({"q": "x"}),
                server: None,
            }),
        });
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "type": "session_tool_call",
                "address": "spawned-f-0001",
                "run_id": "run-f",
                "id": "tc-1",
                "name": "memory_search",
                "arguments": {"q": "x"},
            })
        );
    }

    #[test]
    fn live_summary_has_no_outcome_yet() {
        let info = live_info("spawned-h-0005", SessionState::Running);
        let summary = summary_from_live(&info);
        assert_eq!(summary.outcome, None);
        assert_eq!(summary.error, None);
        assert_eq!(summary.error_details, None);
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
        let paths: Vec<String> = (0..=residuum_workspace::watch::MAX_CHANGES_PER_FRAME)
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
}
