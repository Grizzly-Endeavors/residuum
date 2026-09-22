//! Agent sessions HTTP API: the sessions sidebar's listing and transcripts.
//!
//! - `GET /api/sessions` — live sessions plus a page of completed runs.
//! - `GET /api/sessions/runs/{run_id}/transcript` — one run's transcript in
//!   the chat-history message shape.
//!
//! Live updates arrive over the WebSocket as `session_*` frames; these
//! endpoints give the sidebar its starting state and a finished run's
//! history.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};

use crate::background::registry::{SessionCategory, SessionRegistry};
use crate::background::store::{RunCursor, SessionStore, is_valid_run_id};
use crate::gateway::protocol::{SessionListResponse, SessionSummary};
use crate::gateway::sessions::{summary_from_live, summary_from_record};
use crate::inference::Message;
use crate::memory::recent_messages::RecentMessage;
use crate::memory::types::Visibility;

/// Completed runs per page when the request doesn't say.
const DEFAULT_PAGE_SIZE: usize = 50;

/// Most completed runs one request may ask for.
const MAX_PAGE_SIZE: usize = 200;

/// Shared state for the sessions API.
#[derive(Clone)]
pub(crate) struct SessionsApiState {
    /// Live sessions.
    pub registry: Arc<SessionRegistry>,
    /// Every run's durable record.
    pub store: Arc<SessionStore>,
    /// Timezone transcript timestamps are rendered in, matching chat history.
    pub tz: chrono_tz::Tz,
}

/// Build the sessions API router.
pub(crate) fn sessions_api_router(state: SessionsApiState) -> axum::Router {
    use axum::routing::get;
    axum::Router::new()
        .route("/api/sessions", get(api_sessions_list))
        .route(
            "/api/sessions/runs/{run_id}/transcript",
            get(api_session_transcript),
        )
        .with_state(state)
}

/// An API failure: a status code and a plain-language explanation.
type ApiError = (StatusCode, String);

/// Query parameters for `GET /api/sessions`.
#[derive(Debug, Deserialize)]
pub(crate) struct SessionListQuery {
    /// Only sessions in this category (`scheduled`, `external`, `spawned`).
    #[serde(default)]
    category: Option<SessionCategory>,
    /// Continue after a previous page: its `next_cursor`.
    #[serde(default)]
    before: Option<String>,
    /// Completed runs per page (1 to 200, default 50).
    #[serde(default)]
    limit: Option<usize>,
}

/// `GET /api/sessions` — every live session plus one page of completed runs,
/// both newest first and both filtered by `category` when given.
///
/// Live sessions come from the registry and are always returned in full
/// (there are only ever as many as are running or lingering idle).
/// Completed runs come from the session store, `limit` at a time; pass the
/// response's `next_cursor` back as `before` for the next page. A run that
/// finished between the store write and leaving the registry appears only in
/// `live`.
///
/// # Errors
/// `400` for a malformed cursor or an out-of-range limit (an unknown
/// category is rejected by the query parser), `500` if the session store
/// can't be read.
pub(crate) async fn api_sessions_list(
    State(state): State<SessionsApiState>,
    Query(query): Query<SessionListQuery>,
) -> Result<Json<SessionListResponse>, ApiError> {
    let limit = query.limit.unwrap_or(DEFAULT_PAGE_SIZE);
    if !(1..=MAX_PAGE_SIZE).contains(&limit) {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("limit must be between 1 and {MAX_PAGE_SIZE}"),
        ));
    }
    let before = match query.before.as_deref() {
        Some(raw) => Some(RunCursor::decode(raw).ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                "before is not a cursor this server issued".to_string(),
            )
        })?),
        None => None,
    };

    let mut live_infos = state.registry.list_live();
    live_infos.retain(|info| query.category.is_none_or(|c| info.category == c));
    live_infos.reverse();
    let live: Vec<SessionSummary> = live_infos.iter().map(summary_from_live).collect();

    let page = state
        .store
        .list_completed_runs(
            query.category.as_ref().map(SessionCategory::as_str),
            before.as_ref(),
            limit,
        )
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "failed to list completed session runs");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "couldn't read the session history; check the server logs".to_string(),
            )
        })?;

    let completed = page
        .runs
        .iter()
        .filter(|record| !live.iter().any(|l| l.run_id == record.run_id))
        .filter_map(summary_from_record)
        .collect();

    Ok(Json(SessionListResponse {
        live,
        completed,
        next_cursor: page.next.map(|cursor| cursor.encode()),
    }))
}

/// `GET /api/sessions/runs/{run_id}/transcript` response.
#[derive(Debug, Serialize)]
pub(crate) struct SessionTranscriptResponse {
    /// The run, with its current state when it is still live.
    session: SessionSummary,
    /// Its transcript so far, in the same shape `GET /api/chat/history`
    /// returns. Runs don't record per-message times, so every message
    /// carries the run's start time.
    messages: Vec<RecentMessage>,
}

/// `GET /api/sessions/runs/{run_id}/transcript` — one run's transcript.
///
/// A live run's transcript is read from its incremental transcript (current
/// to the last message produced); a completed run's from its final record.
///
/// # Errors
/// `400` for a run id that can't be one this store issued, `404` for a run
/// that doesn't exist, `500` if its record can't be read.
pub(crate) async fn api_session_transcript(
    State(state): State<SessionsApiState>,
    Path(run_id): Path<String>,
) -> Result<Json<SessionTranscriptResponse>, ApiError> {
    if !is_valid_run_id(&run_id) {
        return Err((StatusCode::BAD_REQUEST, "invalid run id".to_string()));
    }

    let (session, transcript) = if let Some(info) = state.registry.get_by_run_id(&run_id) {
        let transcript = state
            .store
            .read_incremental_transcript(&info.run_id, info.started_at)
            .await;
        (summary_from_live(&info), transcript)
    } else {
        let found = state.store.read_run(&run_id).await.map_err(|e| {
            tracing::warn!(error = %e, run_id = %run_id, "failed to read session run");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "couldn't read this session's transcript; check the server logs".to_string(),
            )
        })?;
        let Some((record, transcript)) = found else {
            return Err((StatusCode::NOT_FOUND, format!("no run {run_id}")));
        };
        let summary = summary_from_record(&record).ok_or_else(|| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "this session's record is damaged; check the server logs".to_string(),
            )
        })?;
        (summary, transcript)
    };

    let timestamp = session.started_at.with_timezone(&state.tz).naive_local();
    let messages = transcript
        .into_iter()
        .map(|message: Message| RecentMessage {
            message,
            timestamp,
            visibility: Visibility::User,
        })
        .collect();
    Ok(Json(SessionTranscriptResponse { session, messages }))
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, TimeZone, Utc};
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::background::registry::{MAIN_ADDRESS, SessionInfo, SessionState};
    use crate::bus::{EventTrigger, SessionAddress};
    use crate::gateway::protocol::SessionListResponse;

    fn info(address: &str, run_id: &str, trigger: EventTrigger, minutes: i64) -> SessionInfo {
        SessionInfo {
            address: SessionAddress::from(address),
            run_id: run_id.to_string(),
            category: SessionCategory::from_trigger(&trigger),
            trigger,
            source_label: "agent:researcher".to_string(),
            state: SessionState::Running,
            spawner: Some(SessionAddress::from(MAIN_ADDRESS)),
            depth: 1,
            purpose: "research".to_string(),
            agent_skill: None,
            model_tier: crate::config::BackgroundModelTier::Medium,
            conversation_target: None,
            started_at: Utc.with_ymd_and_hms(2026, 9, 20, 12, 0, 0).unwrap()
                + Duration::minutes(minutes),
        }
    }

    fn state() -> (SessionsApiState, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let state = SessionsApiState {
            registry: Arc::new(SessionRegistry::new()),
            store: Arc::new(SessionStore::new(dir.path().to_path_buf())),
            tz: chrono_tz::UTC,
        };
        (state, dir)
    }

    async fn complete(state: &SessionsApiState, info: &SessionInfo) {
        state.store.begin_run(info).await;
        state
            .store
            .complete_run(
                info,
                "completed",
                vec![Message::user("go"), Message::assistant("done", None)],
                Some("ep-001".to_string()),
            )
            .await
            .unwrap();
    }

    async fn list(
        state: &SessionsApiState,
        category: Option<SessionCategory>,
        before: Option<String>,
        limit: Option<usize>,
    ) -> Result<SessionListResponse, ApiError> {
        api_sessions_list(
            State(state.clone()),
            Query(SessionListQuery {
                category,
                before,
                limit,
            }),
        )
        .await
        .map(|Json(body)| body)
    }

    #[tokio::test]
    async fn lists_live_and_completed_runs_newest_first() {
        let (state, _dir) = state();
        // Completed runs across two days, including one run left `running`
        // on disk (live) that must not show up as completed.
        complete(
            &state,
            &info("spawned-a-0001", "run-a", EventTrigger::Agent, 0),
        )
        .await;
        complete(
            &state,
            &info("scheduled-b-0001", "run-b", EventTrigger::Pulse, 60 * 24),
        )
        .await;
        complete(
            &state,
            &info("spawned-c-0001", "run-c", EventTrigger::Agent, 60 * 24 + 5),
        )
        .await;
        let live = info(
            "spawned-live-0001",
            "run-live",
            EventTrigger::Agent,
            60 * 48,
        );
        state.store.begin_run(&live).await;
        let _rx = state
            .registry
            .register(live.clone(), CancellationToken::new())
            .unwrap();

        let body = list(&state, None, None, None).await.unwrap();
        let live_ids: Vec<&str> = body.live.iter().map(|s| s.run_id.as_str()).collect();
        assert_eq!(live_ids, vec!["run-live"]);
        let completed_ids: Vec<&str> = body.completed.iter().map(|s| s.run_id.as_str()).collect();
        assert_eq!(completed_ids, vec!["run-c", "run-b", "run-a"]);
        assert!(body.next_cursor.is_none(), "everything fit on one page");

        let first = body.completed.first().unwrap();
        assert_eq!(first.state, SessionState::Completed);
        assert_eq!(first.episode_id.as_deref(), Some("ep-001"));
        assert!(first.completed_at.is_some());
    }

    #[tokio::test]
    async fn paginates_completed_runs_with_a_cursor() {
        let (state, _dir) = state();
        for (i, minutes) in [0_i64, 1, 2, 60 * 24, 60 * 24 + 1].into_iter().enumerate() {
            complete(
                &state,
                &info(
                    &format!("spawned-p-{i:04}"),
                    &format!("run-p{i}"),
                    EventTrigger::Agent,
                    minutes,
                ),
            )
            .await;
        }

        let mut seen = Vec::new();
        let mut cursor = None;
        loop {
            let page = list(&state, None, cursor.clone(), Some(2)).await.unwrap();
            assert!(page.completed.len() <= 2);
            seen.extend(page.completed.into_iter().map(|s| s.run_id));
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        assert_eq!(seen, vec!["run-p4", "run-p3", "run-p2", "run-p1", "run-p0"]);
    }

    #[tokio::test]
    async fn filters_by_category() {
        let (state, _dir) = state();
        complete(
            &state,
            &info("spawned-a-0001", "run-a", EventTrigger::Agent, 0),
        )
        .await;
        complete(
            &state,
            &info("scheduled-b-0001", "run-b", EventTrigger::Pulse, 1),
        )
        .await;
        let live = info("scheduled-live-0001", "run-live", EventTrigger::Action, 2);
        let _rx = state
            .registry
            .register(live, CancellationToken::new())
            .unwrap();

        let body = list(&state, Some(SessionCategory::Scheduled), None, None)
            .await
            .unwrap();
        let live_ids: Vec<&str> = body.live.iter().map(|s| s.run_id.as_str()).collect();
        let completed_ids: Vec<&str> = body.completed.iter().map(|s| s.run_id.as_str()).collect();
        assert_eq!(live_ids, vec!["run-live"]);
        assert_eq!(completed_ids, vec!["run-b"]);

        let spawned = list(&state, Some(SessionCategory::Spawned), None, None)
            .await
            .unwrap();
        assert!(spawned.live.is_empty());
        assert_eq!(spawned.completed.len(), 1);
    }

    #[tokio::test]
    async fn rejects_bad_limits_and_cursors() {
        let (state, _dir) = state();
        let zero = list(&state, None, None, Some(0)).await.unwrap_err();
        assert_eq!(zero.0, StatusCode::BAD_REQUEST);
        let huge = list(&state, None, None, Some(MAX_PAGE_SIZE + 1))
            .await
            .unwrap_err();
        assert_eq!(huge.0, StatusCode::BAD_REQUEST);
        let bad_cursor = list(&state, None, Some("../../etc".to_string()), None)
            .await
            .unwrap_err();
        assert_eq!(bad_cursor.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn completed_transcript_uses_the_chat_history_message_shape() {
        let (state, _dir) = state();
        let run = info("spawned-t-0001", "run-t", EventTrigger::Agent, 0);
        complete(&state, &run).await;

        let Json(body) = api_session_transcript(State(state.clone()), Path("run-t".to_string()))
            .await
            .unwrap();
        assert_eq!(body.session.run_id, "run-t");
        assert_eq!(body.session.state, SessionState::Completed);
        assert_eq!(
            serde_json::to_value(&body.messages).unwrap(),
            serde_json::json!([
                {"role": "user", "content": "go", "timestamp": "2026-09-20T12:00", "visibility": "user"},
                {"role": "assistant", "content": "done", "timestamp": "2026-09-20T12:00", "visibility": "user"},
            ])
        );
    }

    #[tokio::test]
    async fn live_transcript_reads_the_incremental_transcript_and_live_state() {
        let (state, _dir) = state();
        let mut run = info("spawned-l-0001", "run-l", EventTrigger::Agent, 0);
        run.state = SessionState::Idle;
        state.store.begin_run(&run).await;
        state
            .store
            .append_transcript(&run.run_id, run.started_at, &[Message::user("hello")])
            .await;
        let _rx = state
            .registry
            .register(run, CancellationToken::new())
            .unwrap();

        let Json(body) = api_session_transcript(State(state.clone()), Path("run-l".to_string()))
            .await
            .unwrap();
        assert_eq!(body.session.state, SessionState::Idle);
        assert_eq!(body.messages.len(), 1);
        assert_eq!(body.messages.first().unwrap().message.content, "hello");
    }

    #[tokio::test]
    async fn transcript_rejects_unsafe_ids_and_reports_missing_runs() {
        let (state, _dir) = state();
        let unsafe_id = api_session_transcript(State(state.clone()), Path("../x".to_string()))
            .await
            .unwrap_err();
        assert_eq!(unsafe_id.0, StatusCode::BAD_REQUEST);

        let missing = api_session_transcript(State(state.clone()), Path("run-nope".to_string()))
            .await
            .unwrap_err();
        assert_eq!(missing.0, StatusCode::NOT_FOUND);
    }
}
