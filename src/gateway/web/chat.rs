//! Chat history and session-usage HTTP handlers.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::Json;
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::agent::usage::{SessionUsageTotals, load_session_usage_totals};
use crate::inference::Message;
use crate::memory::episode_store::{latest_episode_id, previous_episode_id, read_episode_jsonl};
use crate::memory::recent_messages::{RecentMessage, load_recent_messages};
use crate::memory::types::Visibility;

use super::ConfigApiState;

/// Query parameters for `GET /api/chat/history`.
#[derive(Debug, Deserialize)]
pub(super) struct ChatHistoryQuery {
    /// If set, fetch this specific episode instead of the live recent messages.
    #[serde(default)]
    pub(super) episode: Option<String>,
}

/// One segment of chat history returned by `GET /api/chat/history`.
///
/// `next_cursor`, if present, is the episode ID the client should pass back
/// as `?episode=<id>` to load the next-older segment.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum ChatHistorySegment {
    /// Live, uncompressed messages from `recent_messages.json`.
    Recent {
        messages: Vec<RecentMessage>,
        next_cursor: Option<String>,
    },
    /// A single archived episode, synthesized as `RecentMessage`s so the
    /// frontend can render them through the existing pipeline.
    Episode {
        episode_id: String,
        date: NaiveDate,
        messages: Vec<RecentMessage>,
        next_cursor: Option<String>,
    },
}

/// `GET /api/chat/history` — return a segment of chat history.
///
/// With no query params, returns the live `Recent` segment plus a cursor
/// pointing at the newest episode on disk (for the frontend's lazy-load).
///
/// With `?episode=ep-NNN`, returns that episode's transcript wrapped as
/// `RecentMessage`s plus a cursor to the next-older episode. Returns 404
/// when the episode does not exist.
pub(super) async fn api_chat_history(
    State(state): State<ConfigApiState>,
    Query(params): Query<ChatHistoryQuery>,
) -> Result<Json<ChatHistorySegment>, StatusCode> {
    let Some(memory_dir) = &state.memory_dir else {
        return Ok(Json(ChatHistorySegment::Recent {
            messages: Vec::new(),
            next_cursor: None,
        }));
    };
    let episodes_dir = memory_dir.join("episodes");

    match params.episode {
        None => {
            let recent_path = memory_dir.join("recent_messages.json");
            let messages = load_recent_messages(&recent_path).await.map_err(|err| {
                tracing::warn!(
                    error = %err,
                    path = %recent_path.display(),
                    "failed to load recent messages — refusing to silently return empty history",
                );
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
            let next_cursor = latest_episode_id(&episodes_dir).await.map_err(|err| {
                tracing::warn!(
                    error = %err,
                    path = %episodes_dir.display(),
                    "failed to scan episodes directory",
                );
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
            Ok(Json(ChatHistorySegment::Recent {
                messages,
                next_cursor,
            }))
        }
        Some(episode_id) => {
            let path = crate::memory::episode_store::find_episode_path(&episodes_dir, &episode_id)
                .map_err(|err| {
                    tracing::warn!(error = %err, episode = %episode_id, "failed to locate episode");
                    StatusCode::INTERNAL_SERVER_ERROR
                })?;
            let Some(path) = path else {
                return Err(StatusCode::NOT_FOUND);
            };

            let (meta, raw_messages) = read_episode_jsonl(&path).await.map_err(|err| {
                tracing::warn!(error = %err, episode = %episode_id, "failed to read episode transcript");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;

            let timestamp = meta.date.and_hms_opt(0, 0, 0).unwrap_or_default();
            let messages = raw_messages
                .into_iter()
                .map(|message| wrap_episode_message(message, timestamp))
                .collect();

            let next_cursor =
                previous_episode_id(&episodes_dir, &meta.id)
                    .await
                    .map_err(|err| {
                        tracing::warn!(
                            error = %err,
                            episode = %meta.id,
                            path = %episodes_dir.display(),
                            "failed to walk to previous episode",
                        );
                        StatusCode::INTERNAL_SERVER_ERROR
                    })?;

            Ok(Json(ChatHistorySegment::Episode {
                episode_id: meta.id,
                date: meta.date,
                messages,
                next_cursor,
            }))
        }
    }
}

/// `GET /api/usage` — the main agent's cumulative session token usage, for
/// the chat footer to render correctly on load or reconnect without
/// waiting for the next model call.
///
/// Reads the same on-disk totals the running agent writes through to after
/// every model call (see `crate::agent::usage::MainUsageSink`), the same
/// way `GET /api/chat/history` reads `recent_messages.json` rather than
/// reaching into the live agent — this HTTP layer never holds a reference
/// to it. Returns the zero default in setup mode (no memory dir yet).
pub(super) async fn api_usage(State(state): State<ConfigApiState>) -> Json<SessionUsageTotals> {
    let Some(memory_dir) = &state.memory_dir else {
        return Json(SessionUsageTotals::default());
    };
    let path = memory_dir.join("usage_totals.json");
    Json(load_session_usage_totals(&path).await)
}

/// Synthesize a `RecentMessage` wrapper around a raw episode `Message`.
///
/// Episode JSONL stores only raw `Message` values, so we fabricate metadata
/// using the episode's date (at 00:00). Visibility is always `User` because
/// the original per-message visibility was not recorded in the transcript.
fn wrap_episode_message(message: Message, timestamp: chrono::NaiveDateTime) -> RecentMessage {
    RecentMessage {
        message,
        timestamp,
        visibility: Visibility::User,
        turn_id: None,
    }
}
