//! Chat history and session-usage HTTP handlers.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::Json;
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::agent::usage::{SessionUsageTotals, load_session_usage_totals};
use crate::inference::{Message, readable_thinking};
use crate::memory::episode_store::{latest_episode_id, previous_episode_id, read_episode_jsonl};
use crate::memory::recent_messages::{RecentMessage, load_recent_messages};
use crate::memory::types::Visibility;

use super::AgentFilesState;

/// Query parameters for `GET /api/agents/{name}/chat/history`.
#[derive(Debug, Deserialize)]
pub(super) struct ChatHistoryQuery {
    /// If set, fetch this specific episode instead of the live recent messages.
    #[serde(default)]
    pub(super) episode: Option<String>,
}

/// One message of chat history, as the web client reads it.
///
/// A [`RecentMessage`] with the model's reasoning reduced to readable text:
/// the opaque signatures and encrypted data a provider needs replayed
/// belong to the model's conversation, never to a client.
#[derive(Debug, Serialize)]
pub(crate) struct HistoryMessage {
    /// The message, without its reasoning blocks.
    #[serde(flatten)]
    pub(crate) message: Message,
    /// The readable reasoning behind an assistant message, one entry per
    /// block that has text. Absent when the model produced none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) thinking: Vec<String>,
    /// When the message was recorded.
    #[serde(with = "crate::time::minute_format")]
    pub(crate) timestamp: chrono::NaiveDateTime,
    /// Whether the message came from a user-visible or background turn.
    pub(crate) visibility: Visibility,
    /// The correlation id of the turn that produced the message, if known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) turn_id: Option<String>,
}

impl From<RecentMessage> for HistoryMessage {
    fn from(recent: RecentMessage) -> Self {
        let thinking = readable_thinking(&recent.message.thinking)
            .into_iter()
            .map(str::to_string)
            .collect();
        let message = Message {
            thinking: Vec::new(),
            ..recent.message
        };
        Self {
            message,
            thinking,
            timestamp: recent.timestamp,
            visibility: recent.visibility,
            turn_id: recent.turn_id,
        }
    }
}

/// One segment of chat history returned by `GET /api/agents/{name}/chat/history`.
///
/// `next_cursor`, if present, is the episode ID the client should pass back
/// as `?episode=<id>` to load the next-older segment.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum ChatHistorySegment {
    /// Live, uncompressed messages from `recent_messages.json`.
    Recent {
        messages: Vec<HistoryMessage>,
        next_cursor: Option<String>,
    },
    /// A single archived episode, synthesized as history messages so the
    /// frontend can render them through the existing pipeline.
    Episode {
        episode_id: String,
        date: NaiveDate,
        messages: Vec<HistoryMessage>,
        next_cursor: Option<String>,
    },
}

/// `GET /api/agents/{name}/chat/history` — return a segment of chat history.
///
/// With no query params, returns the live `Recent` segment plus a cursor
/// pointing at the newest episode on disk (for the frontend's lazy-load).
///
/// With `?episode=ep-NNN`, returns that episode's transcript wrapped as
/// `RecentMessage`s plus a cursor to the next-older episode. Returns 404
/// when the episode does not exist.
pub(super) async fn api_chat_history(
    State(state): State<AgentFilesState>,
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
                messages: messages.into_iter().map(HistoryMessage::from).collect(),
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
                .map(|message| HistoryMessage::from(wrap_episode_message(message, timestamp)))
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

/// `GET /api/agents/{name}/usage` — the main agent's cumulative session token usage, for
/// the conversation size to render correctly on load or reconnect without
/// waiting for the next model call.
///
/// Reads the same on-disk totals the running agent writes through to after
/// every model call (see `crate::agent::usage::MainUsageSink`), the same
/// way `GET /api/agents/{name}/chat/history` reads `recent_messages.json` rather than
/// reaching into the live agent — this HTTP layer never holds a reference
/// to it. Returns the zero default in setup mode (no memory dir yet).
pub(super) async fn api_usage(State(state): State<AgentFilesState>) -> Json<SessionUsageTotals> {
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

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "test code uses indexing for clarity"
)]
mod tests {
    use axum::extract::{Query, State};

    use super::*;
    use crate::inference::ThinkingBlock;
    use crate::memory::recent_messages::append_recent_messages;

    /// An assistant message whose reasoning the provider needs replayed:
    /// readable text, a signature to send back, and a block that is only
    /// encrypted.
    fn assistant_with_reasoning() -> Message {
        Message::assistant("The answer is 42.".to_string(), None).with_thinking(vec![
            ThinkingBlock {
                text: "First, the question.".to_string(),
                signature: Some("sig-first-secret".to_string()),
                redacted: None,
            },
            ThinkingBlock {
                text: String::new(),
                signature: None,
                redacted: Some("encrypted-secret".to_string()),
            },
            ThinkingBlock {
                text: "Then the arithmetic.".to_string(),
                signature: Some("sig-second-secret".to_string()),
                redacted: None,
            },
        ])
    }

    async fn history_json(memory_dir: std::path::PathBuf) -> serde_json::Value {
        let state = AgentFilesState::for_test(memory_dir.clone(), Some(memory_dir));
        let Json(segment) =
            api_chat_history(State(state), Query(ChatHistoryQuery { episode: None }))
                .await
                .unwrap();
        serde_json::to_value(segment).unwrap()
    }

    #[tokio::test]
    async fn history_exposes_readable_thinking_and_never_the_replay_data() {
        let dir = tempfile::tempdir().unwrap();
        append_recent_messages(
            &dir.path().join("recent_messages.json"),
            &[Message::user("what is it?"), assistant_with_reasoning()],
            Visibility::User,
            chrono_tz::UTC,
            Some("turn-1"),
        )
        .await
        .unwrap();

        let json = history_json(dir.path().to_path_buf()).await;

        let messages = json
            .get("messages")
            .and_then(serde_json::Value::as_array)
            .unwrap();
        let assistant = &messages[1];
        assert_eq!(
            assistant.get("thinking"),
            Some(&serde_json::json!([
                "First, the question.",
                "Then the arithmetic."
            ])),
            "one entry per block that has readable text"
        );
        assert_eq!(
            assistant.get("content"),
            Some(&serde_json::json!("The answer is 42."))
        );
        let wire = json.to_string();
        for secret in ["sig-first-secret", "sig-second-secret", "encrypted-secret"] {
            assert!(
                !wire.contains(secret),
                "{secret} must not reach a client: {wire}"
            );
        }
        assert!(
            messages.first().unwrap().get("thinking").is_none(),
            "a message without reasoning has no thinking field"
        );
    }

    #[tokio::test]
    async fn recent_messages_keep_the_reasoning_the_provider_needs_replayed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recent_messages.json");
        append_recent_messages(
            &path,
            &[assistant_with_reasoning()],
            Visibility::User,
            chrono_tz::UTC,
            Some("turn-1"),
        )
        .await
        .unwrap();

        let loaded = load_recent_messages(&path).await.unwrap();

        let blocks = &loaded.first().unwrap().message.thinking;
        assert_eq!(blocks, &assistant_with_reasoning().thinking);
    }

    #[test]
    fn a_whitespace_only_block_is_not_readable_reasoning() {
        let recent = RecentMessage {
            message: Message::assistant("ok".to_string(), None).with_thinking(vec![
                ThinkingBlock::text("  \n"),
                ThinkingBlock::text("real"),
            ]),
            timestamp: chrono::NaiveDateTime::default(),
            visibility: Visibility::User,
            turn_id: None,
        };

        let history = HistoryMessage::from(recent);

        assert_eq!(history.thinking, ["real"]);
        assert!(history.message.thinking.is_empty());
    }
}
