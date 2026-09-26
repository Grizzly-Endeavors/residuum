//! Agent sessions HTTP API: the sessions sidebar's listing and transcripts,
//! and the start, stop, and message endpoints workbench artifacts use.
//!
//! - `GET /api/sessions` — live sessions plus a page of completed runs.
//! - `GET /api/sessions/runs/{run_id}/transcript` — one run's transcript in
//!   the chat-history message shape.
//! - `POST /api/sessions` — start an `artifact` session for the artifact
//!   named by the request's identity header.
//! - `POST /api/sessions/{address}/stop` — stop a live session.
//! - `POST /api/sessions/{address}/messages` — send a session a message.
//!
//! Live updates arrive over the WebSocket as `session_*` frames; these
//! endpoints give the sidebar its starting state and a finished run's
//! history. Stop and message share their rules with the WebSocket's
//! `session_stop` and `session_send_message` commands
//! (`crate::gateway::sessions`).

use std::sync::Arc;

use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::background::messaging::AgentMessenger;
use crate::background::registry::{
    MAIN_DEPTH, SessionCategory, SessionRegistry, artifact_sender_address, generate_address,
};
use crate::background::store::{RunCursor, RunFilter, SessionStore, is_valid_run_id};
use crate::bus::{EventTrigger, Publisher, SkillName, SpawnRequestEvent, topics};
use crate::config::BackgroundModelTier;
use crate::gateway::protocol::{
    SessionCommandErrorCode, SessionDeliveryOutcome, SessionListResponse, SessionSummary,
};
use crate::gateway::sessions::{
    SessionCommandError, SessionMessageAuthor, send_session_message, stop_session,
    summary_from_live, summary_from_record,
};
use crate::gateway::web::artifact_identity::{ARTIFACT_HEADER, artifact_identity};
use crate::inference::Message;
use crate::memory::recent_messages::RecentMessage;
use crate::memory::types::Visibility;
use crate::skills::SharedSkillState;
use crate::workbench::is_valid_artifact_name;

/// Completed runs per page when the request doesn't say.
const DEFAULT_PAGE_SIZE: usize = 50;

/// Shared state for the sessions API.
#[derive(Clone)]
pub(crate) struct SessionsApiState {
    /// Live sessions.
    pub registry: Arc<SessionRegistry>,
    /// Every run's durable record.
    pub store: Arc<SessionStore>,
    /// Timezone transcript timestamps are rendered in, matching chat history.
    pub tz: chrono_tz::Tz,
    /// Delivers messages to sessions, as the sidebar's own messages go.
    pub messenger: Arc<AgentMessenger>,
    /// Publishes spawn requests for sessions an artifact starts.
    pub publisher: Publisher,
    /// The skill index, to refuse a start naming a skill that doesn't exist.
    pub skill_state: SharedSkillState,
}

/// Build the sessions API router.
pub(crate) fn sessions_api_router(state: SessionsApiState) -> axum::Router {
    use axum::routing::{get, post};
    axum::Router::new()
        .route(
            "/api/sessions",
            get(api_sessions_list).post(api_session_start),
        )
        .route(
            "/api/sessions/runs/{run_id}/transcript",
            get(api_session_transcript),
        )
        .route("/api/sessions/{address}/stop", post(api_session_stop))
        .route(
            "/api/sessions/{address}/messages",
            post(api_session_message),
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
    /// Only runs of the session at this address.
    #[serde(default)]
    address: Option<String>,
    /// Only sessions this workbench artifact started.
    #[serde(default)]
    artifact: Option<String>,
    /// Continue after a previous page: its `next_cursor`.
    #[serde(default)]
    before: Option<String>,
    /// Completed runs per page (at least 1, default 50). No upper cap.
    #[serde(default)]
    limit: Option<usize>,
}

/// `GET /api/sessions` — every live session plus one page of completed runs,
/// both newest first and both filtered by `category`, `address`, and
/// `artifact` when given. `artifact` keeps only the sessions that artifact
/// started (trigger `Artifact(<name>)`), not sessions those spawned in turn.
///
/// Live sessions come from the registry and are always returned in full
/// (there are only ever as many as are running or lingering idle).
/// Completed runs come from the session store, `limit` at a time; pass the
/// response's `next_cursor` back as `before` for the next page. A run that
/// finished between the store write and leaving the registry appears only in
/// `live`.
///
/// # Errors
/// `400` for a malformed cursor, a `limit` of zero, or an invalid artifact
/// name (an unknown category is rejected by the query parser), `500` if the
/// session store can't be read.
pub(crate) async fn api_sessions_list(
    State(state): State<SessionsApiState>,
    Query(query): Query<SessionListQuery>,
) -> Result<Json<SessionListResponse>, ApiError> {
    let limit = query.limit.unwrap_or(DEFAULT_PAGE_SIZE);
    if limit == 0 {
        return Err((
            StatusCode::BAD_REQUEST,
            "limit must be at least 1".to_string(),
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
    let artifact = query.artifact.as_deref();
    if artifact.is_some_and(|name| !is_valid_artifact_name(name)) {
        return Err((
            StatusCode::BAD_REQUEST,
            "artifact must be an artifact name, like \"wiki-graph\"".to_string(),
        ));
    }
    // Store records don't keep the trigger: an artifact session's category
    // plus its `artifact:<name>` source label identify it exactly.
    let artifact_label = artifact.map(|name| artifact_sender_address(name).to_string());
    let category = match (artifact, query.category) {
        (None, category) => category,
        (Some(_), None | Some(SessionCategory::Artifact)) => Some(SessionCategory::Artifact),
        (Some(_), Some(_)) => {
            // Only `artifact` sessions have an artifact trigger.
            return Ok(Json(SessionListResponse {
                live: Vec::new(),
                completed: Vec::new(),
                next_cursor: None,
            }));
        }
    };

    let mut live_infos = state.registry.list_live();
    live_infos.retain(|info| {
        category.is_none_or(|c| info.category == c)
            && query
                .address
                .as_deref()
                .is_none_or(|a| info.address.as_ref() == a)
            && artifact.is_none_or(
                |name| matches!(&info.trigger, EventTrigger::Artifact(started_by) if started_by == name),
            )
    });
    live_infos.reverse();
    let live: Vec<SessionSummary> = live_infos.iter().map(summary_from_live).collect();

    let page = state
        .store
        .list_completed_runs(
            RunFilter {
                category: category.as_ref().map(SessionCategory::as_str),
                address: query.address.as_deref(),
                source_label: artifact_label.as_deref(),
            },
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

/// Body of an error from the start, stop, and message endpoints. `code` is
/// the WebSocket session command's error code, where one applies.
#[derive(Debug, Serialize)]
struct CommandErrorBody {
    error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<SessionCommandErrorCode>,
}

/// A JSON error response with no session command code.
fn json_error(status: StatusCode, message: impl Into<String>) -> Response {
    (
        status,
        Json(CommandErrorBody {
            error: message.into(),
            code: None,
        }),
    )
        .into_response()
}

/// The HTTP status a session command failure answers with.
fn command_error_status(code: SessionCommandErrorCode) -> StatusCode {
    match code {
        SessionCommandErrorCode::InvalidRequest => StatusCode::BAD_REQUEST,
        SessionCommandErrorCode::UnknownAddress | SessionCommandErrorCode::NotLive => {
            StatusCode::NOT_FOUND
        }
        SessionCommandErrorCode::Busy => StatusCode::CONFLICT,
        SessionCommandErrorCode::DeliveryFailed => StatusCode::BAD_GATEWAY,
    }
}

/// A session command failure as `{ error, code }` with the status per code.
fn command_error_response(e: SessionCommandError) -> Response {
    (
        command_error_status(e.code),
        Json(CommandErrorBody {
            error: e.message,
            code: Some(e.code),
        }),
    )
        .into_response()
}

/// `POST /api/sessions` request body.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SessionStartRequest {
    /// The session's task.
    prompt: String,
    /// Extra context the session reads before the task.
    #[serde(default)]
    context: Option<String>,
    /// Skill to activate for the session.
    #[serde(default)]
    skill: Option<String>,
    /// Model tier: `small`, `medium` (default), or `large`.
    #[serde(default)]
    model: Option<String>,
}

/// `POST /api/sessions` response.
#[derive(Debug, Serialize)]
pub(crate) struct SessionStartResponse {
    /// The new session's address. Its run id arrives in the
    /// `session_started` frame for this address.
    address: String,
}

/// `POST /api/sessions` — start an `artifact` session for the artifact named
/// by the request's identity header.
///
/// The session is a fork of the main agent like a spawned one, at depth 1
/// with no spawner. Its output stays with the artifact: it is never relayed
/// to the main agent or routed to the inbox. Answers `202` with the address
/// as soon as the spawn request is published; the run itself starts
/// asynchronously and announces itself with `session_started`.
///
/// # Errors
/// `400` without a valid artifact identity header (only artifacts start
/// sessions this way), for a malformed body, a blank prompt, an unknown
/// model tier, or an unknown skill; `503` if the spawn request can't be
/// published.
pub(crate) async fn api_session_start(
    State(state): State<SessionsApiState>,
    headers: HeaderMap,
    body: Result<Json<SessionStartRequest>, JsonRejection>,
) -> Response {
    let artifact = match artifact_identity(&headers) {
        Ok(Some(name)) => name,
        Ok(None) => {
            return json_error(
                StatusCode::BAD_REQUEST,
                format!(
                    "starting a session needs the {ARTIFACT_HEADER} header: sessions are \
                     started by workbench artifacts, through residuum.sessions.start"
                ),
            );
        }
        Err(message) => return json_error(StatusCode::BAD_REQUEST, message),
    };
    let Json(body) = match body {
        Ok(body) => body,
        Err(rejection) => {
            return json_error(
                StatusCode::BAD_REQUEST,
                format!(
                    "the body must be JSON like {{\"prompt\": \"...\"}}: {}",
                    rejection.body_text()
                ),
            );
        }
    };
    if body.prompt.trim().is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "prompt must not be empty");
    }
    let model_tier = match body.model.as_deref() {
        None => BackgroundModelTier::Medium,
        Some(raw) => match raw.parse::<BackgroundModelTier>() {
            Ok(tier) => tier,
            Err(message) => return json_error(StatusCode::BAD_REQUEST, message),
        },
    };
    if let Some(skill) = body.skill.as_deref()
        && let Err(message) = check_skill(&state.skill_state, skill).await
    {
        return json_error(StatusCode::BAD_REQUEST, message);
    }

    let trigger = EventTrigger::Artifact(artifact.clone());
    let address = generate_address(&trigger, &artifact);
    let context = artifact_session_context(&artifact, body.context.as_deref());
    let event = SpawnRequestEvent {
        address: address.clone(),
        skill: body.skill.as_deref().map(SkillName::from),
        source_label: artifact_sender_address(&artifact).to_string(),
        prompt: body.prompt,
        context: Some(context),
        source: trigger,
        model_tier,
        spawner: None,
        depth: MAIN_DEPTH + 1,
        hop_count: 0,
        sender: None,
        conversation: None,
        inbound: None,
        images: Vec::new(),
        overlap: None,
    };
    if let Err(e) = state.publisher.publish(topics::Background, event).await {
        tracing::warn!(artifact = %artifact, address = %address, error = %e, "failed to publish an artifact's session start");
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Couldn't start the session because Residuum is shutting down or restarting. Try again shortly.",
        );
    }
    tracing::info!(
        artifact = %artifact,
        address = %address,
        skill = body.skill.as_deref().unwrap_or("none"),
        model_tier = %model_tier,
        "artifact started a session"
    );
    (
        StatusCode::ACCEPTED,
        Json(SessionStartResponse {
            address: address.to_string(),
        }),
    )
        .into_response()
}

/// Refuse a skill name the session couldn't activate, before the fork would
/// fail on it.
async fn check_skill(skill_state: &SharedSkillState, skill: &str) -> Result<(), String> {
    if skill.eq_ignore_ascii_case("main") {
        return Err(
            "\"main\" is reserved; name a skill, or leave skill out to run on the prompt alone"
                .to_string(),
        );
    }
    let state = skill_state.lock().await;
    if state.index().find_by_name(skill).is_some() {
        return Ok(());
    }
    let available: Vec<&str> = state
        .index()
        .entries()
        .iter()
        .map(|e| e.name.as_str())
        .collect();
    Err(format!(
        "unknown skill \"{skill}\". Available: {}",
        if available.is_empty() {
            "none".to_string()
        } else {
            available.join(", ")
        }
    ))
}

/// The context an artifact session reads before its task: who started it and
/// where its responses go, then whatever context the artifact supplied.
fn artifact_session_context(artifact: &str, extra: Option<&str>) -> String {
    let framing = format!(
        "[This session was started by the workbench artifact \"{artifact}\". Your responses \
         are shown to that artifact, not to the main conversation.]"
    );
    match extra.map(str::trim) {
        Some(extra) if !extra.is_empty() => format!("{framing}\n\n{extra}"),
        _ => framing,
    }
}

/// `POST /api/sessions/{address}/stop` response.
#[derive(Debug, Serialize)]
pub(crate) struct SessionStopResponse {
    /// The session being stopped.
    address: String,
}

/// `POST /api/sessions/{address}/stop` — stop a live session, as the
/// WebSocket's `session_stop` command does. Serves any session, not only
/// artifact sessions.
///
/// # Errors
/// `404` (`not_live`) when no live session at `address` can still be
/// stopped, `400` (`invalid_request`) for `main`.
pub(crate) async fn api_session_stop(
    State(state): State<SessionsApiState>,
    Path(address): Path<String>,
) -> Response {
    match stop_session(&state.registry, &address) {
        Ok(()) => (StatusCode::ACCEPTED, Json(SessionStopResponse { address })).into_response(),
        Err(e) => command_error_response(e),
    }
}

/// `POST /api/sessions/{address}/messages` request body.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SessionMessageRequest {
    /// The message text.
    content: String,
}

/// `POST /api/sessions/{address}/messages` response.
#[derive(Debug, Serialize)]
pub(crate) struct SessionMessageResponse {
    /// Where the message landed.
    outcome: SessionDeliveryOutcome,
}

/// `POST /api/sessions/{address}/messages` — send a session a message, with
/// the WebSocket's `session_send_message` delivery rules. With an artifact
/// identity header the session sees the message as that artifact's;
/// without one, as the owner's. Serves any session, not only artifact
/// sessions.
///
/// # Errors
/// `{ error, code }` with the status per code: `invalid_request` `400` (bad
/// body, blank content, `main`, a malformed identity header),
/// `unknown_address` `404`, `busy` `409`, `delivery_failed` `502`.
pub(crate) async fn api_session_message(
    State(state): State<SessionsApiState>,
    Path(address): Path<String>,
    headers: HeaderMap,
    body: Result<Json<SessionMessageRequest>, JsonRejection>,
) -> Response {
    let invalid = |message: String| {
        command_error_response(SessionCommandError {
            code: SessionCommandErrorCode::InvalidRequest,
            message,
        })
    };
    let author = match artifact_identity(&headers) {
        Ok(Some(name)) => SessionMessageAuthor::Artifact(name),
        Ok(None) => SessionMessageAuthor::Owner,
        Err(message) => return invalid(message),
    };
    let Json(body) = match body {
        Ok(body) => body,
        Err(rejection) => {
            return invalid(format!(
                "the body must be JSON like {{\"content\": \"...\"}}: {}",
                rejection.body_text()
            ));
        }
    };
    match send_session_message(&state.messenger, &address, body.content, &author).await {
        Ok(outcome) => (StatusCode::OK, Json(SessionMessageResponse { outcome })).into_response(),
        Err(e) => command_error_response(e),
    }
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
            usage: crate::agent::usage::SessionUsageTotals::default(),
            overlap: None,
        }
    }

    /// What a test's [`SessionsApiState`] needs kept alive: its store's
    /// directory and the bus its publisher and messenger use.
    struct Fixture {
        _dir: tempfile::TempDir,
        bus: crate::bus::BusHandle,
    }

    fn state() -> (SessionsApiState, Fixture) {
        state_with_skills(crate::skills::SkillIndex::default())
    }

    fn state_with_skills(index: crate::skills::SkillIndex) -> (SessionsApiState, Fixture) {
        let dir = tempfile::tempdir().unwrap();
        let bus = crate::bus::spawn_broker();
        let registry = Arc::new(SessionRegistry::new());
        let store = Arc::new(SessionStore::new(dir.path().to_path_buf()));
        let messenger = Arc::new(AgentMessenger::new(
            Arc::clone(&registry),
            bus.publisher(),
            Arc::clone(&store),
            crate::background::HopLimits { soft: 8, hard: 32 },
        ));
        let state = SessionsApiState {
            registry,
            store,
            tz: chrono_tz::UTC,
            messenger,
            publisher: bus.publisher(),
            skill_state: crate::skills::SkillState::new_shared(index, vec![]),
        };
        (state, Fixture { _dir: dir, bus })
    }

    async fn complete(state: &SessionsApiState, info: &SessionInfo) {
        state.store.begin_run(info).await;
        state
            .store
            .complete_run(
                info,
                "completed",
                &crate::bus::AgentResultStatus::Completed,
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
                address: None,
                artifact: None,
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
    async fn filters_by_address() {
        let (state, _dir) = state();
        complete(
            &state,
            &info("spawned-a-0001", "run-a1", EventTrigger::Agent, 0),
        )
        .await;
        complete(
            &state,
            &info("spawned-b-0001", "run-b", EventTrigger::Agent, 1),
        )
        .await;
        complete(
            &state,
            &info("spawned-a-0001", "run-a2", EventTrigger::Agent, 2),
        )
        .await;
        let live = info("spawned-b-0001", "run-b-live", EventTrigger::Agent, 3);
        let _rx = state
            .registry
            .register(live, CancellationToken::new())
            .unwrap();

        let Json(body) = api_sessions_list(
            State(state.clone()),
            Query(SessionListQuery {
                category: None,
                address: Some("spawned-a-0001".to_string()),
                artifact: None,
                before: None,
                limit: None,
            }),
        )
        .await
        .unwrap();
        assert!(body.live.is_empty(), "the live run is at another address");
        let completed_ids: Vec<&str> = body.completed.iter().map(|s| s.run_id.as_str()).collect();
        assert_eq!(completed_ids, vec!["run-a2", "run-a1"]);
    }

    #[tokio::test]
    async fn rejects_bad_limits_and_cursors() {
        let (state, _dir) = state();
        let zero = list(&state, None, None, Some(0)).await.unwrap_err();
        assert_eq!(zero.0, StatusCode::BAD_REQUEST);
        // A limit above the old 200 ceiling is honoured, not rejected.
        let huge = list(&state, None, None, Some(500)).await.unwrap();
        assert!(huge.completed.len() <= 500);
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

    // ── Artifact sessions: start, stop, message, filter ──────────────────

    use axum::body::Body;
    use axum::http::{HeaderValue, Request};
    use tower::ServiceExt;

    use crate::agent::interrupt::Interrupt;
    use crate::background::registry::ResumePoint;

    /// A response's status and JSON body.
    struct Answer {
        status: StatusCode,
        body: serde_json::Value,
    }

    impl Answer {
        /// A top-level string field of the body, or `""` when absent.
        fn text(&self, key: &str) -> &str {
            self.body
                .get(key)
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
        }
    }

    fn artifact_headers(name: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(ARTIFACT_HEADER, HeaderValue::from_str(name).unwrap());
        headers
    }

    fn artifact_info(address: &str, run_id: &str, artifact: &str, minutes: i64) -> SessionInfo {
        let mut session = info(
            address,
            run_id,
            EventTrigger::Artifact(artifact.to_string()),
            minutes,
        );
        session.source_label = format!("artifact:{artifact}");
        session.spawner = None;
        session
    }

    async fn answer(resp: Response) -> Answer {
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        Answer {
            status,
            body: serde_json::from_slice(&bytes).unwrap(),
        }
    }

    async fn start(
        state: &SessionsApiState,
        headers: HeaderMap,
        request: serde_json::Value,
    ) -> Answer {
        let request = serde_json::from_value::<SessionStartRequest>(request).unwrap();
        answer(api_session_start(State(state.clone()), headers, Ok(Json(request))).await).await
    }

    async fn stop(state: &SessionsApiState, address: &str) -> Answer {
        answer(api_session_stop(State(state.clone()), Path(address.to_string())).await).await
    }

    async fn message(
        state: &SessionsApiState,
        address: &str,
        headers: HeaderMap,
        content: &str,
    ) -> Answer {
        answer(
            api_session_message(
                State(state.clone()),
                Path(address.to_string()),
                headers,
                Ok(Json(SessionMessageRequest {
                    content: content.to_string(),
                })),
            )
            .await,
        )
        .await
    }

    async fn next_spawn(
        spawns: &mut crate::bus::Subscriber<SpawnRequestEvent>,
    ) -> SpawnRequestEvent {
        tokio::time::timeout(std::time::Duration::from_secs(1), spawns.recv())
            .await
            .expect("a spawn request should be published")
            .unwrap()
            .unwrap()
    }

    async fn subscribe_spawns(fx: &Fixture) -> crate::bus::Subscriber<SpawnRequestEvent> {
        fx.bus.subscribe(topics::Background).await.unwrap()
    }

    #[tokio::test]
    async fn start_without_the_identity_header_is_refused() {
        let (state, fx) = state();
        let mut spawns = subscribe_spawns(&fx).await;
        let prompt = serde_json::json!({ "prompt": "summarize the wiki" });

        let missing = start(&state, HeaderMap::new(), prompt.clone()).await;
        assert_eq!(missing.status, StatusCode::BAD_REQUEST);
        assert!(
            missing.text("error").contains(ARTIFACT_HEADER),
            "got {}",
            missing.body
        );

        let malformed = start(&state, artifact_headers("Not A Name"), prompt).await;
        assert_eq!(malformed.status, StatusCode::BAD_REQUEST);

        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), spawns.recv())
                .await
                .is_err(),
            "a refused start must not spawn anything"
        );
    }

    #[tokio::test]
    async fn start_publishes_an_artifact_session_spawn_and_answers_its_address() {
        let (state, fx) = state();
        let mut spawns = subscribe_spawns(&fx).await;

        let started = start(
            &state,
            artifact_headers("wiki-graph"),
            serde_json::json!({ "prompt": "write a page about otters", "context": "use the notes folder" }),
        )
        .await;
        assert_eq!(started.status, StatusCode::ACCEPTED);
        let address = started.text("address");
        assert!(address.starts_with("artifact-wiki-graph-"), "got {address}");

        let spawn = next_spawn(&mut spawns).await;
        assert_eq!(spawn.address.as_ref(), address);
        assert!(matches!(&spawn.source, EventTrigger::Artifact(name) if name == "wiki-graph"));
        assert_eq!(
            SessionCategory::from_trigger(&spawn.source),
            SessionCategory::Artifact
        );
        assert_eq!(spawn.source_label, "artifact:wiki-graph");
        assert_eq!(spawn.spawner, None, "an artifact session has no spawner");
        assert_eq!(spawn.depth, 1);
        assert_eq!(spawn.hop_count, 0);
        assert_eq!(spawn.model_tier, BackgroundModelTier::Medium);
        assert_eq!(spawn.skill, None);
        assert_eq!(spawn.prompt, "write a page about otters");
        let context = spawn.context.unwrap();
        assert!(
            context.contains("workbench artifact \"wiki-graph\""),
            "got {context}"
        );
        assert!(context.ends_with("use the notes folder"), "got {context}");
    }

    #[tokio::test]
    async fn start_validates_model_and_skill() {
        let skills_root = tempfile::tempdir().unwrap();
        let researcher = skills_root.path().join("researcher");
        std::fs::create_dir_all(&researcher).unwrap();
        std::fs::write(
            researcher.join("SKILL.md"),
            "---\nname: researcher\ndescription: Researches things carefully.\n---\nDo research.\n",
        )
        .unwrap();
        let index = crate::skills::SkillIndex::scan(&[skills_root.path().to_path_buf()])
            .await
            .unwrap();
        let (state, fx) = state_with_skills(index);
        let mut spawns = subscribe_spawns(&fx).await;
        let headers = || artifact_headers("wiki");

        let bad_model = start(
            &state,
            headers(),
            serde_json::json!({ "prompt": "go", "model": "huge" }),
        )
        .await;
        assert_eq!(bad_model.status, StatusCode::BAD_REQUEST);
        assert!(bad_model.text("error").contains("huge"));

        let bad_skill = start(
            &state,
            headers(),
            serde_json::json!({ "prompt": "go", "skill": "astrologer" }),
        )
        .await;
        assert_eq!(bad_skill.status, StatusCode::BAD_REQUEST);
        assert!(
            bad_skill.text("error").contains("researcher"),
            "an unknown skill lists the ones that exist, got {}",
            bad_skill.body
        );

        let blank = start(&state, headers(), serde_json::json!({ "prompt": "   " })).await;
        assert_eq!(blank.status, StatusCode::BAD_REQUEST);

        let good = start(
            &state,
            headers(),
            serde_json::json!({ "prompt": "go", "skill": "researcher", "model": "large" }),
        )
        .await;
        assert_eq!(good.status, StatusCode::ACCEPTED);
        let spawn = next_spawn(&mut spawns).await;
        assert_eq!(spawn.skill.as_ref().map(AsRef::as_ref), Some("researcher"));
        assert_eq!(spawn.model_tier, BackgroundModelTier::Large);
    }

    #[tokio::test]
    async fn stop_matches_the_websocket_command_with_http_statuses() {
        let (state, _fx) = state();
        let token = CancellationToken::new();
        let _rx = state
            .registry
            .register(
                artifact_info("artifact-wiki-0001", "run-w", "wiki", 0),
                token.clone(),
            )
            .unwrap();

        let stopping = stop(&state, "artifact-wiki-0001").await;
        assert_eq!(stopping.status, StatusCode::ACCEPTED);
        assert_eq!(stopping.text("address"), "artifact-wiki-0001");
        assert!(
            token.is_cancelled(),
            "the session's run is signalled to stop"
        );

        let not_live = stop(&state, "spawned-ghost-0000").await;
        assert_eq!(not_live.status, StatusCode::NOT_FOUND);
        assert_eq!(not_live.text("code"), "not_live");

        let main = stop(&state, MAIN_ADDRESS).await;
        assert_eq!(main.status, StatusCode::BAD_REQUEST);
        assert_eq!(main.text("code"), "invalid_request");
    }

    #[tokio::test]
    async fn message_is_attributed_to_the_artifact_with_the_header_and_the_owner_without() {
        let (state, _fx) = state();
        let mut rx = state
            .registry
            .register(
                artifact_info("artifact-wiki-0001", "run-w", "wiki", 0),
                CancellationToken::new(),
            )
            .unwrap();

        let from_artifact = message(
            &state,
            "artifact-wiki-0001",
            artifact_headers("wiki"),
            "refresh",
        )
        .await;
        assert_eq!(from_artifact.status, StatusCode::OK);
        assert_eq!(from_artifact.text("outcome"), "live");
        let Some(Interrupt::AgentMessage(artifact_msg)) = rx.try_recv().ok() else {
            panic!("the session should have received the artifact's message");
        };
        assert_eq!(artifact_msg.artifact_sender(), Some("wiki"));
        assert!(
            artifact_msg
                .format_for_agent()
                .starts_with("[Message from the workbench artifact \"wiki\""),
            "the session sees the artifact, not the owner"
        );

        let from_owner = message(&state, "artifact-wiki-0001", HeaderMap::new(), "hi").await;
        assert_eq!(from_owner.status, StatusCode::OK);
        assert_eq!(from_owner.text("outcome"), "live");
        let Some(Interrupt::AgentMessage(owner_msg)) = rx.try_recv().ok() else {
            panic!("the session should have received the owner's message");
        };
        assert_eq!(
            owner_msg.from.as_ref(),
            crate::background::registry::OWNER_ADDRESS
        );
        assert!(
            owner_msg
                .format_for_agent()
                .starts_with("[Message from the owner via the web UI")
        );
    }

    #[tokio::test]
    async fn message_resumes_a_completed_session() {
        let (state, fx) = state();
        let mut spawns = subscribe_spawns(&fx).await;
        state
            .registry
            .record_resume_point(
                &SessionAddress::from("artifact-wiki-0001"),
                ResumePoint {
                    previous_run_id: "run-old".to_string(),
                    previous_episode_id: None,
                    trigger: EventTrigger::Artifact("wiki".to_string()),
                    source_label: "artifact:wiki".to_string(),
                    agent_skill: None,
                    model_tier: BackgroundModelTier::Medium,
                    spawner: None,
                    depth: 1,
                    conversation_target: None,
                    recorded_at: chrono::Utc::now(),
                },
            )
            .await;

        let resumed = message(
            &state,
            "artifact-wiki-0001",
            artifact_headers("wiki"),
            "again",
        )
        .await;
        assert_eq!(resumed.status, StatusCode::OK);
        assert_eq!(resumed.text("outcome"), "resumed");
        let spawn = next_spawn(&mut spawns).await;
        assert!(
            matches!(&spawn.source, EventTrigger::Artifact(name) if name == "wiki"),
            "a resumed artifact session stays an artifact session"
        );
        assert!(spawn.prompt.contains("workbench artifact \"wiki\""));
    }

    #[tokio::test]
    async fn message_failures_answer_their_code_with_the_status_per_code() {
        let (state, _fx) = state();

        let unknown = message(&state, "spawned-ghost-0000", HeaderMap::new(), "hi").await;
        assert_eq!(unknown.status, StatusCode::NOT_FOUND);
        assert_eq!(unknown.text("code"), "unknown_address");
        assert!(!unknown.text("error").is_empty());

        let blank = message(&state, "spawned-ghost-0000", HeaderMap::new(), "  ").await;
        assert_eq!(blank.status, StatusCode::BAD_REQUEST);
        assert_eq!(blank.text("code"), "invalid_request");

        let to_main = message(&state, MAIN_ADDRESS, HeaderMap::new(), "hi").await;
        assert_eq!(to_main.status, StatusCode::BAD_REQUEST);
        assert_eq!(to_main.text("code"), "invalid_request");

        let bad_identity = message(
            &state,
            "spawned-ghost-0000",
            artifact_headers("Bad Name"),
            "hi",
        )
        .await;
        assert_eq!(bad_identity.status, StatusCode::BAD_REQUEST);
        assert_eq!(bad_identity.text("code"), "invalid_request");

        // A live session whose interrupt channel is full is busy.
        let _rx = state
            .registry
            .register(
                artifact_info("artifact-full-0001", "run-f", "full", 0),
                CancellationToken::new(),
            )
            .unwrap();
        for _ in 0..crate::background::registry::INTERRUPT_CHANNEL_CAPACITY {
            let filled = message(&state, "artifact-full-0001", HeaderMap::new(), "fill").await;
            assert_eq!(filled.status, StatusCode::OK);
        }
        let busy = message(
            &state,
            "artifact-full-0001",
            HeaderMap::new(),
            "one too many",
        )
        .await;
        assert_eq!(busy.status, StatusCode::CONFLICT);
        assert_eq!(busy.text("code"), "busy");

        assert_eq!(
            command_error_status(SessionCommandErrorCode::DeliveryFailed),
            StatusCode::BAD_GATEWAY
        );
        assert_eq!(
            command_error_status(SessionCommandErrorCode::NotLive),
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn artifact_filter_returns_only_that_artifacts_sessions() {
        let (state, _fx) = state();
        complete(
            &state,
            &artifact_info("artifact-wiki-0001", "run-w1", "wiki", 0),
        )
        .await;
        complete(
            &state,
            &artifact_info("artifact-chart-0001", "run-c1", "chart", 1),
        )
        .await;
        complete(
            &state,
            &info("spawned-a-0001", "run-s1", EventTrigger::Agent, 2),
        )
        .await;
        let _live_wiki = state
            .registry
            .register(
                artifact_info("artifact-wiki-0002", "run-w2", "wiki", 3),
                CancellationToken::new(),
            )
            .unwrap();
        let _live_chart = state
            .registry
            .register(
                artifact_info("artifact-chart-0002", "run-c2", "chart", 4),
                CancellationToken::new(),
            )
            .unwrap();

        let query = |artifact: &str, category: Option<SessionCategory>| SessionListQuery {
            category,
            address: None,
            artifact: Some(artifact.to_string()),
            before: None,
            limit: None,
        };
        let Json(wiki) = api_sessions_list(State(state.clone()), Query(query("wiki", None)))
            .await
            .unwrap();
        let live: Vec<&str> = wiki.live.iter().map(|s| s.run_id.as_str()).collect();
        let completed: Vec<&str> = wiki.completed.iter().map(|s| s.run_id.as_str()).collect();
        assert_eq!(live, vec!["run-w2"]);
        assert_eq!(completed, vec!["run-w1"]);
        assert!(
            wiki.completed
                .iter()
                .all(|s| s.category == SessionCategory::Artifact),
            "the artifact category round-trips through the store"
        );

        let Json(mismatched) = api_sessions_list(
            State(state.clone()),
            Query(query("wiki", Some(SessionCategory::Spawned))),
        )
        .await
        .unwrap();
        assert!(mismatched.live.is_empty() && mismatched.completed.is_empty());

        let err = api_sessions_list(State(state.clone()), Query(query("../x", None)))
            .await
            .unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    async fn post(app: &axum::Router, request: Request<Body>) -> Answer {
        let resp = app.clone().oneshot(request).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        Answer {
            status,
            body: serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        }
    }

    #[tokio::test]
    async fn routes_reach_the_start_stop_and_message_handlers() {
        let (state, _fx) = state();
        let app = sessions_api_router(state);
        let json_post = |path: &str, artifact: Option<&str>, body: &'static str| {
            let mut builder = Request::post(path).header("content-type", "application/json");
            if let Some(name) = artifact {
                builder = builder.header("X-Residuum-Artifact", name);
            }
            builder.body(Body::from(body)).unwrap()
        };

        let started = post(
            &app,
            json_post("/api/sessions", Some("wiki"), r#"{"prompt":"go"}"#),
        )
        .await;
        assert_eq!(started.status, StatusCode::ACCEPTED);

        let misspelled = post(
            &app,
            json_post("/api/sessions", Some("wiki"), r#"{"promt":"go"}"#),
        )
        .await;
        assert_eq!(
            misspelled.status,
            StatusCode::BAD_REQUEST,
            "a misspelled field is refused"
        );
        assert!(!misspelled.text("error").is_empty());

        let stopped = post(
            &app,
            Request::post("/api/sessions/artifact-ghost-0000/stop")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(stopped.status, StatusCode::NOT_FOUND);

        let messaged = post(
            &app,
            json_post(
                "/api/sessions/artifact-ghost-0000/messages",
                None,
                r#"{"content":"hi"}"#,
            ),
        )
        .await;
        assert_eq!(messaged.status, StatusCode::NOT_FOUND);
        assert_eq!(messaged.text("code"), "unknown_address");

        let transcript = post(
            &app,
            Request::get("/api/sessions/runs/run-nope/transcript")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(
            transcript.status,
            StatusCode::NOT_FOUND,
            "the transcript route still resolves beside the address routes"
        );
    }
}
