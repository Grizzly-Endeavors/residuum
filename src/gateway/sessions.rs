//! Web-facing view of agent sessions: turning registry entries, store
//! records, and bus events into protocol types, and carrying out the
//! sessions sidebar's send-message and stop commands.

use crate::background::messaging::{AgentMessenger, DeliveryOutcome, SendError};
use crate::background::registry::{
    MAIN_ADDRESS, OWNER_ADDRESS, SessionCategory, SessionInfo, SessionRegistry, SessionState,
};
use crate::background::store::RunRecord;
use crate::bus::{AgentResultStatus, SessionAddress, SessionEvent, SessionEventKind};
use crate::gateway::protocol::{
    ServerMessage, SessionCommandErrorCode, SessionDeliveryOutcome, SessionRunStatus,
    SessionSummary,
};

/// Category label a sidebar message carries as its sender's category.
const OWNER_CATEGORY: &str = "owner";

/// Summarize a live run from its registry entry.
#[must_use]
pub(crate) fn summary_from_live(info: &SessionInfo) -> SessionSummary {
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
    }
}

/// Summarize a run from its store record. `None` (logged) when the record
/// names a category or state this build doesn't know, which would mean a
/// corrupted or hand-edited record.
///
/// A record's state is only as fresh as its last write: a live run's record
/// says `running` whatever its current state, so callers prefer
/// [`summary_from_live`] for a run that is still in the registry.
#[must_use]
pub(crate) fn summary_from_record(record: &RunRecord) -> Option<SessionSummary> {
    let (Some(category), Some(state)) = (
        SessionCategory::from_label(&record.category),
        SessionState::from_label(&record.state),
    ) else {
        tracing::warn!(
            run_id = %record.run_id,
            category = %record.category,
            state = %record.state,
            "session run record has an unrecognized category or state, leaving it out"
        );
        return None;
    };
    Some(SessionSummary {
        address: record.address.clone(),
        run_id: record.run_id.clone(),
        category,
        source_label: record.source_label.clone(),
        state,
        spawner: record.spawner.clone(),
        depth: record.depth,
        purpose: record.purpose.clone(),
        started_at: record.started_at,
        completed_at: record.completed_at,
        episode_id: record.episode_id.clone(),
        interrupted: record.interrupted,
    })
}

/// Translate a bus session event into the WebSocket frame clients receive.
#[must_use]
pub(crate) fn session_event_to_server_message(event: SessionEvent) -> ServerMessage {
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
            let (status, error) = match status {
                AgentResultStatus::Completed => (SessionRunStatus::Completed, None),
                AgentResultStatus::Cancelled => (SessionRunStatus::Cancelled, None),
                AgentResultStatus::Failed { error } => (SessionRunStatus::Failed, Some(error)),
            };
            ServerMessage::SessionCompleted {
                address,
                run_id,
                status,
                error,
                episode_id,
            }
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
        SessionEventKind::ToolCall(call) => ServerMessage::SessionToolCall {
            address,
            run_id,
            id: call.tool_call_id,
            name: call.name,
            arguments: call.arguments,
        },
        SessionEventKind::ToolResult(result) => ServerMessage::SessionToolResult {
            address,
            run_id,
            tool_call_id: result.tool_call_id,
            name: result.name,
            output: result.output,
            is_error: result.is_error,
        },
        SessionEventKind::Intermediate { content } => ServerMessage::SessionBroadcastResponse {
            address,
            run_id,
            content,
        },
        SessionEventKind::Response { turn_id, content } => ServerMessage::SessionResponse {
            address,
            run_id,
            turn_id,
            content,
        },
        SessionEventKind::Error { message } => ServerMessage::SessionError {
            address,
            run_id,
            message,
        },
    }
}

/// A session command the sidebar sent could not be carried out.
#[derive(Debug, Clone)]
pub(crate) struct SessionCommandError {
    /// Machine-readable reason.
    pub code: SessionCommandErrorCode,
    /// Explanation suitable to show the user.
    pub message: String,
}

impl SessionCommandError {
    fn new(code: SessionCommandErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// Deliver the owner's sidebar message to the session at `address`.
///
/// Goes through the same messenger every agent message does, as hop count 0
/// (the owner's input originates outside the agent system), so the normal
/// delivery rules apply: an interrupt while running, a new turn while idle,
/// a new run once completed.
///
/// # Errors
/// Returns a [`SessionCommandError`] when the request is unusable (empty
/// content, or `main`, which is messaged through the normal chat), when no
/// session has ever run at `address`, when the session is too busy to take
/// another message, or when delivery fails.
pub(crate) async fn send_owner_message(
    messenger: &AgentMessenger,
    address: &str,
    content: String,
) -> Result<SessionDeliveryOutcome, SessionCommandError> {
    if content.trim().is_empty() {
        return Err(SessionCommandError::new(
            SessionCommandErrorCode::InvalidRequest,
            "Type a message before sending.",
        ));
    }
    if address == MAIN_ADDRESS {
        return Err(SessionCommandError::new(
            SessionCommandErrorCode::InvalidRequest,
            "The main agent is messaged from the main chat, not the sessions sidebar.",
        ));
    }

    let outcome = messenger
        .send(
            address,
            SessionAddress::from(OWNER_ADDRESS),
            OWNER_CATEGORY.to_string(),
            content,
            0,
        )
        .await;

    match outcome {
        Ok(DeliveryOutcome::Live(_)) => Ok(SessionDeliveryOutcome::Live),
        Ok(DeliveryOutcome::Resumed(_)) => Ok(SessionDeliveryOutcome::Resumed),
        Ok(DeliveryOutcome::Queued(_)) => Ok(SessionDeliveryOutcome::Queued),
        Ok(DeliveryOutcome::Unknown) => Err(SessionCommandError::new(
            SessionCommandErrorCode::UnknownAddress,
            format!("There's no session called {address}. It may have been from before a restart."),
        )),
        Ok(DeliveryOutcome::Main) => {
            // Unreachable: `main` is rejected above. Reported rather than
            // assumed, so a routing change can't silently misdeliver.
            tracing::error!(address, "sidebar message to a session was routed to main");
            Err(SessionCommandError::new(
                SessionCommandErrorCode::DeliveryFailed,
                "The message went to the main agent instead of the session.",
            ))
        }
        Err(SendError::Busy(_)) => Err(SessionCommandError::new(
            SessionCommandErrorCode::Busy,
            format!("{address} is busy and can't take another message yet. Try again shortly."),
        )),
        Err(e @ (SendError::PublishFailed(_) | SendError::HopLimitExceeded { .. })) => {
            tracing::warn!(error = %e, address, "failed to deliver sidebar message to session");
            Err(SessionCommandError::new(
                SessionCommandErrorCode::DeliveryFailed,
                format!("Couldn't deliver the message to {address}. Try again."),
            ))
        }
    }
}

/// Stop the live session at `address`, as the sidebar's stop button does.
///
/// # Errors
/// Returns a [`SessionCommandError`] when `address` is `main` or doesn't name
/// a live session that can still be stopped (it may already be finishing).
pub(crate) fn stop_session(
    registry: &SessionRegistry,
    address: &str,
) -> Result<(), SessionCommandError> {
    if address == MAIN_ADDRESS {
        return Err(SessionCommandError::new(
            SessionCommandErrorCode::InvalidRequest,
            "The main agent can't be stopped from the sessions sidebar.",
        ));
    }
    if registry.stop(&SessionAddress::from(address)) {
        tracing::info!(address, "session stop requested from the web UI");
        Ok(())
    } else {
        Err(SessionCommandError::new(
            SessionCommandErrorCode::NotLive,
            format!("{address} isn't running, so there's nothing to stop."),
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::Utc;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::agent::interrupt::Interrupt;
    use crate::background::registry::ResumePoint;
    use crate::background::store::SessionStore;
    use crate::bus::{EventTrigger, ToolCallEvent};

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
            model_tier: crate::config::BackgroundModelTier::Medium,
            started_at: Utc::now(),
        }
    }

    fn messenger(
        registry: &Arc<SessionRegistry>,
    ) -> (AgentMessenger, crate::bus::BusHandle, tempfile::TempDir) {
        let bus = crate::bus::spawn_broker();
        let dir = tempfile::tempdir().unwrap();
        let messenger = AgentMessenger::new(
            Arc::clone(registry),
            bus.publisher(),
            Arc::new(SessionStore::new(dir.path().to_path_buf())),
            crate::background::HopLimits { soft: 8, hard: 32 },
        );
        (messenger, bus, dir)
    }

    #[tokio::test]
    async fn owner_message_reaches_a_live_session_at_hop_zero_labelled_as_the_owner() {
        let registry = Arc::new(SessionRegistry::new());
        let mut rx = registry
            .register(
                live_info("spawned-a-0001", SessionState::Idle),
                CancellationToken::new(),
            )
            .unwrap();
        let (messenger, _bus, _dir) = messenger(&registry);

        let outcome = send_owner_message(&messenger, "spawned-a-0001", "status?".to_string())
            .await
            .unwrap();
        assert_eq!(outcome, SessionDeliveryOutcome::Live);

        let Some(Interrupt::AgentMessage(msg)) = rx.try_recv().ok() else {
            panic!("the session should have received an agent message");
        };
        assert_eq!(msg.hop_count, 0, "sidebar input is hop count 0");
        assert_eq!(msg.from.as_ref(), OWNER_ADDRESS);
        assert!(
            msg.format_for_agent()
                .starts_with("[Message from the owner via the web UI"),
            "the session should see the message as the owner's"
        );
    }

    #[tokio::test]
    async fn owner_message_to_a_completed_session_resumes_it() {
        let registry = Arc::new(SessionRegistry::new());
        registry.record_resume_point(
            &SessionAddress::from("spawned-b-0001"),
            ResumePoint {
                previous_run_id: "run-old".to_string(),
                previous_episode_id: None,
                trigger: EventTrigger::Agent,
                source_label: "agent:researcher".to_string(),
                agent_skill: None,
                model_tier: crate::config::BackgroundModelTier::Medium,
                spawner: Some(SessionAddress::from(MAIN_ADDRESS)),
                depth: 1,
            },
        );
        let (messenger, bus, _dir) = messenger(&registry);
        let mut spawns: crate::bus::Subscriber<crate::bus::SpawnRequestEvent> =
            bus.subscribe(crate::bus::topics::Background).await.unwrap();

        let outcome = send_owner_message(&messenger, "spawned-b-0001", "one more".to_string())
            .await
            .unwrap();
        assert_eq!(outcome, SessionDeliveryOutcome::Resumed);
        let spawn = spawns.recv().await.unwrap().unwrap();
        assert_eq!(spawn.address.as_ref(), "spawned-b-0001");
        assert_eq!(spawn.hop_count, 0);
    }

    #[tokio::test]
    async fn owner_message_errors_are_specific() {
        let registry = Arc::new(SessionRegistry::new());
        let (messenger, _bus, _dir) = messenger(&registry);

        let unknown = send_owner_message(&messenger, "spawned-nope-0000", "hi".to_string())
            .await
            .unwrap_err();
        assert_eq!(unknown.code, SessionCommandErrorCode::UnknownAddress);

        let empty = send_owner_message(&messenger, "spawned-nope-0000", "   ".to_string())
            .await
            .unwrap_err();
        assert_eq!(empty.code, SessionCommandErrorCode::InvalidRequest);

        let main = send_owner_message(&messenger, MAIN_ADDRESS, "hi".to_string())
            .await
            .unwrap_err();
        assert_eq!(main.code, SessionCommandErrorCode::InvalidRequest);
    }

    #[tokio::test]
    async fn owner_message_to_a_saturated_session_is_busy() {
        let registry = Arc::new(SessionRegistry::new());
        let _rx = registry
            .register(
                live_info("spawned-full-0001", SessionState::Running),
                CancellationToken::new(),
            )
            .unwrap();
        let (messenger, _bus, _dir) = messenger(&registry);
        for _ in 0..crate::background::registry::INTERRUPT_CHANNEL_CAPACITY {
            send_owner_message(&messenger, "spawned-full-0001", "fill".to_string())
                .await
                .unwrap();
        }

        let busy = send_owner_message(&messenger, "spawned-full-0001", "one too many".to_string())
            .await
            .unwrap_err();
        assert_eq!(busy.code, SessionCommandErrorCode::Busy);
    }

    #[test]
    fn stop_signals_a_live_session_and_refuses_otherwise() {
        let registry = SessionRegistry::new();
        let token = CancellationToken::new();
        let _rx = registry
            .register(
                live_info("spawned-c-0001", SessionState::Running),
                token.clone(),
            )
            .unwrap();

        stop_session(&registry, "spawned-c-0001").unwrap();
        assert!(
            token.is_cancelled(),
            "stop should cancel the session's token"
        );

        let not_live = stop_session(&registry, "spawned-missing-0000").unwrap_err();
        assert_eq!(not_live.code, SessionCommandErrorCode::NotLive);

        let main = stop_session(&registry, MAIN_ADDRESS).unwrap_err();
        assert_eq!(main.code, SessionCommandErrorCode::InvalidRequest);
    }

    #[test]
    fn completed_event_maps_failure_reason_into_its_own_field() {
        let msg = session_event_to_server_message(SessionEvent {
            address: SessionAddress::from("spawned-d-0001"),
            run_id: "run-d".to_string(),
            kind: SessionEventKind::Completed {
                status: AgentResultStatus::Failed {
                    error: "model down".to_string(),
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
    fn record_with_unknown_category_is_left_out() {
        let info = live_info("spawned-g-0001", SessionState::Running);
        let mut record = RunRecord::starting(&info);
        assert!(summary_from_record(&record).is_some());
        record.category = "mystery".to_string();
        assert!(summary_from_record(&record).is_none());
    }
}
