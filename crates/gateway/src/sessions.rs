//! Web-facing view of agent sessions: summarizing store records as protocol
//! types, and carrying out the send-message and stop commands the session
//! panel (over the WebSocket) and workbench artifacts (over HTTP) share.
//! Live registry entries and bus events become frames in
//! `residuum_protocol::frames`.

use residuum_background::messaging::{AgentMessenger, DeliveryOutcome, SendError};
use residuum_background::registry::{SessionRegistry, StopSessionError, stop_named_session};
use residuum_background::store::RunRecord;
use residuum_bus::SessionAddress;
use residuum_bus::session::{
    ARTIFACT_SENDER_CATEGORY, MAIN_ADDRESS, OWNER_ADDRESS, SessionCategory, SessionState,
    artifact_sender_address,
};
use residuum_protocol::{
    SessionCommandErrorCode, SessionDeliveryOutcome, SessionRunStatus, SessionSummary,
};

/// Category label a message from the owner carries as its sender's category.
const OWNER_CATEGORY: &str = "owner";

/// Summarize a run from its store record. `None` (logged) when the record
/// names a category or state this build doesn't know, which would mean a
/// corrupted or hand-edited record.
///
/// A record's state is only as fresh as its last write: a live run's record
/// says `running` whatever its current state, so callers prefer
/// [`residuum_protocol::frames::summary_from_live`] for a run that is still in the registry.
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
    let outcome = record.outcome.as_deref().and_then(|label| {
        let parsed = SessionRunStatus::from_label(label);
        if parsed.is_none() {
            tracing::warn!(
                run_id = %record.run_id,
                outcome = %label,
                "session run record has an unrecognized outcome, showing it as plain 'finished'"
            );
        }
        parsed
    });
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
        usage: record.usage,
        outcome,
        error: record.outcome_error.clone(),
        error_details: record.outcome_error_details.clone(),
        overlap: record.overlap.clone(),
        model: record.model.clone(),
        resolved_model: record.resolved_model.clone(),
    })
}

/// A session command the web UI sent could not be carried out.
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

/// Who a message sent to a session from outside the agent system comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SessionMessageAuthor {
    /// The owner, typing into the web UI (a session panel, or an HTTP
    /// request without an artifact identity).
    Owner,
    /// The workbench artifact with this name, identified by its SDK's header.
    Artifact(String),
}

impl SessionMessageAuthor {
    /// The sender address and category label the message carries.
    fn sender(&self) -> (SessionAddress, String) {
        match self {
            Self::Owner => (
                SessionAddress::from(OWNER_ADDRESS),
                OWNER_CATEGORY.to_string(),
            ),
            Self::Artifact(name) => (
                artifact_sender_address(name),
                ARTIFACT_SENDER_CATEGORY.to_string(),
            ),
        }
    }
}

/// Deliver a message from the owner or a workbench artifact to the session
/// at `address`.
///
/// Goes through the same messenger every agent message does, as hop count 0
/// (the input originates outside the agent system), so the normal delivery
/// rules apply: an interrupt while running, a new turn while idle, a new run
/// once completed. The session sees the message attributed to `author`.
///
/// # Errors
/// Returns a [`SessionCommandError`] when the request is unusable (empty
/// content, or `main`, which is messaged through the normal chat), when no
/// session has ever run at `address`, when the session is too busy to take
/// another message, or when delivery fails.
pub(crate) async fn send_session_message(
    messenger: &AgentMessenger,
    address: &str,
    content: String,
    author: &SessionMessageAuthor,
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
            "The main agent is messaged from the main chat, not as a session.",
        ));
    }

    let (from, from_category) = author.sender();
    let outcome = messenger
        .send(address, from, from_category, content, 0)
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
            tracing::error!(address, "message to a session was routed to main");
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
            tracing::warn!(error = %e, address, author = ?author, "failed to deliver message to session");
            Err(SessionCommandError::new(
                SessionCommandErrorCode::DeliveryFailed,
                format!("Couldn't deliver the message to {address}. Try again."),
            ))
        }
    }
}

/// Stop the live session at `address`, as the Stop button in Activity and the
/// `POST /api/agents/{name}/sessions/{address}/stop` endpoint do.
///
/// # Errors
/// Returns a [`SessionCommandError`] when `address` is `main` or doesn't name
/// a live session that can still be stopped (it may already be finishing).
pub(crate) fn stop_session(
    registry: &SessionRegistry,
    address: &str,
) -> Result<(), SessionCommandError> {
    stop_named_session(registry, address).map_err(|e| {
        let code = match e {
            StopSessionError::Main => SessionCommandErrorCode::InvalidRequest,
            StopSessionError::NotLive { .. } => SessionCommandErrorCode::NotLive,
        };
        SessionCommandError::new(code, e.to_string())
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::Utc;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use residuum_background::registry::ResumePoint;
    use residuum_background::store::SessionStore;
    use residuum_bus::EventTrigger;
    use residuum_bus::interrupt::Interrupt;
    use residuum_bus::session::SessionInfo;

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

    fn messenger(
        registry: &Arc<SessionRegistry>,
    ) -> (AgentMessenger, residuum_bus::BusHandle, tempfile::TempDir) {
        let bus = residuum_bus::spawn_broker();
        let dir = tempfile::tempdir().unwrap();
        let messenger = AgentMessenger::new(
            Arc::clone(registry),
            bus.publisher(),
            Arc::new(SessionStore::new(dir.path().to_path_buf())),
            residuum_agent::hop::HopLimits { soft: 8, hard: 32 },
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

        let outcome = send_session_message(
            &messenger,
            "spawned-a-0001",
            "status?".to_string(),
            &SessionMessageAuthor::Owner,
        )
        .await
        .unwrap();
        assert_eq!(outcome, SessionDeliveryOutcome::Live);

        let Some(Interrupt::AgentMessage(msg)) = rx.try_recv().ok() else {
            panic!("the session should have received an agent message");
        };
        assert_eq!(msg.hop_count, 0, "owner input is hop count 0");
        assert_eq!(msg.from.as_ref(), OWNER_ADDRESS);
        assert!(
            msg.format_for_agent()
                .starts_with("[Message from the owner via the web UI"),
            "the session should see the message as the owner's"
        );
    }

    #[tokio::test]
    async fn artifact_message_reaches_the_session_attributed_to_the_artifact() {
        let registry = Arc::new(SessionRegistry::new());
        let mut rx = registry
            .register(
                live_info("artifact-wiki-0001", SessionState::Idle),
                CancellationToken::new(),
            )
            .unwrap();
        let (messenger, _bus, _dir) = messenger(&registry);

        let outcome = send_session_message(
            &messenger,
            "artifact-wiki-0001",
            "refresh".to_string(),
            &SessionMessageAuthor::Artifact("wiki".to_string()),
        )
        .await
        .unwrap();
        assert_eq!(outcome, SessionDeliveryOutcome::Live);

        let Some(Interrupt::AgentMessage(msg)) = rx.try_recv().ok() else {
            panic!("the session should have received a message");
        };
        assert_eq!(msg.hop_count, 0, "artifact input is hop count 0");
        assert_eq!(msg.from.as_ref(), "artifact:wiki");
        assert_eq!(msg.from_category, "artifact");
        assert_eq!(msg.artifact_sender(), Some("wiki"));
        let text = msg.format_for_agent();
        assert!(
            text.starts_with("[Message from the workbench artifact \"wiki\""),
            "the session should see the message as the artifact's, got {text}"
        );
    }

    #[tokio::test]
    async fn owner_message_to_a_completed_session_resumes_it() {
        let registry = Arc::new(SessionRegistry::new());
        registry
            .record_resume_point(
                &SessionAddress::from("spawned-b-0001"),
                ResumePoint {
                    previous_run_id: "run-old".to_string(),
                    previous_episode_id: None,
                    trigger: EventTrigger::Agent,
                    source_label: "agent:researcher".to_string(),
                    agent_skill: None,
                    model: residuum_config::ModelSelector::MEDIUM,
                    spawner: Some(SessionAddress::from(MAIN_ADDRESS)),
                    depth: 1,
                    conversation_target: None,
                    recorded_at: chrono::Utc::now(),
                },
            )
            .await;
        let (messenger, bus, _dir) = messenger(&registry);
        let mut spawns: residuum_bus::Subscriber<residuum_bus::SpawnRequestEvent> = bus
            .subscribe(residuum_bus::topics::Background)
            .await
            .unwrap();

        let outcome = send_session_message(
            &messenger,
            "spawned-b-0001",
            "one more".to_string(),
            &SessionMessageAuthor::Owner,
        )
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

        let unknown = send_session_message(
            &messenger,
            "spawned-nope-0000",
            "hi".to_string(),
            &SessionMessageAuthor::Owner,
        )
        .await
        .unwrap_err();
        assert_eq!(unknown.code, SessionCommandErrorCode::UnknownAddress);

        let empty = send_session_message(
            &messenger,
            "spawned-nope-0000",
            "   ".to_string(),
            &SessionMessageAuthor::Owner,
        )
        .await
        .unwrap_err();
        assert_eq!(empty.code, SessionCommandErrorCode::InvalidRequest);

        let main = send_session_message(
            &messenger,
            MAIN_ADDRESS,
            "hi".to_string(),
            &SessionMessageAuthor::Owner,
        )
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
        for _ in 0..residuum_background::registry::INTERRUPT_CHANNEL_CAPACITY {
            send_session_message(
                &messenger,
                "spawned-full-0001",
                "fill".to_string(),
                &SessionMessageAuthor::Owner,
            )
            .await
            .unwrap();
        }

        let busy = send_session_message(
            &messenger,
            "spawned-full-0001",
            "one too many".to_string(),
            &SessionMessageAuthor::Owner,
        )
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
    fn a_run_record_keeps_the_model_its_run_used() {
        let mut info = live_info("spawned-g-0001", SessionState::Running);
        info.model = residuum_config::ModelSelector::SMALL;
        info.resolved_model = Some("anthropic/claude-haiku-4-5".to_string());
        let mut record = RunRecord::starting(&info);
        record.state = "completed".to_string();

        let summary = summary_from_record(&record).unwrap();
        assert_eq!(summary.model.as_deref(), Some("small"));
        assert_eq!(
            summary.resolved_model.as_deref(),
            Some("anthropic/claude-haiku-4-5")
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

    #[test]
    fn failed_run_outcome_and_error_survive_into_the_summary() {
        let info = live_info("spawned-h-0001", SessionState::Running);
        let mut record = RunRecord::starting(&info);
        record.state = "completed".to_string();
        record.outcome = Some("failed".to_string());
        record.outcome_error = Some("the model call timed out".to_string());
        record.outcome_error_details =
            Some("connect timeout after 30s: api.example.com:443".to_string());
        let summary = summary_from_record(&record).expect("record should summarize");
        assert_eq!(
            summary.outcome,
            Some(residuum_protocol::SessionRunStatus::Failed),
            "a reloaded record must show its real outcome, not just 'completed' state"
        );
        assert_eq!(
            summary.error.as_deref(),
            Some("the model call timed out"),
            "the failure reason must survive into the summary"
        );
        assert_eq!(
            summary.error_details.as_deref(),
            Some("connect timeout after 30s: api.example.com:443"),
            "the full cause chain must survive into the summary behind the Details disclosure"
        );
    }

    #[test]
    fn a_record_without_error_details_still_summarizes_with_none() {
        // Mirrors a record written before `outcome_error_details` existed —
        // `RunRecord::starting` already defaults it to `None`, so this just
        // pins that `summary_from_record` doesn't fabricate a value.
        let info = live_info("spawned-h-0006", SessionState::Running);
        let mut record = RunRecord::starting(&info);
        record.outcome = Some("failed".to_string());
        record.outcome_error = Some("the model call timed out".to_string());
        let summary = summary_from_record(&record).expect("record should summarize");
        assert_eq!(summary.error_details, None);
    }

    #[test]
    fn stopped_run_outcome_survives_into_the_summary() {
        let info = live_info("spawned-h-0002", SessionState::Running);
        let mut record = RunRecord::starting(&info);
        record.state = "completed".to_string();
        record.outcome = Some("cancelled".to_string());
        let summary = summary_from_record(&record).expect("record should summarize");
        assert_eq!(
            summary.outcome,
            Some(residuum_protocol::SessionRunStatus::Cancelled)
        );
        assert_eq!(summary.error, None);
    }

    #[test]
    fn record_with_no_recorded_outcome_summarizes_with_none() {
        // A record written before this field existed: must not fail to
        // summarize, and must not fabricate an outcome it never recorded.
        let info = live_info("spawned-h-0003", SessionState::Running);
        let record = RunRecord::starting(&info);
        let summary = summary_from_record(&record).expect("record should summarize");
        assert_eq!(summary.outcome, None);
        assert_eq!(summary.error, None);
    }

    #[test]
    fn record_with_unrecognized_outcome_falls_back_to_none_rather_than_dropping_the_record() {
        let info = live_info("spawned-h-0004", SessionState::Running);
        let mut record = RunRecord::starting(&info);
        record.outcome = Some("mystery".to_string());
        let summary = summary_from_record(&record).expect(
            "an unrecognized outcome must not \
             drop the whole record, unlike an unrecognized category or state",
        );
        assert_eq!(summary.outcome, None);
    }
}
