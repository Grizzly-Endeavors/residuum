//! Chat history and session-usage HTTP handlers.

use std::path::Path;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::Json;
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use residuum_agent::usage::load_session_usage_totals;
use residuum_bus::usage::SessionUsageTotals;
use residuum_config::WorkspaceLayout;
use residuum_inference::{Message, Role, readable_thinking};
use residuum_memory::episode_store::{older_main_episode_id, read_episode_jsonl};
use residuum_memory::recent_messages::{RecentMessage, load_recent_messages};
use residuum_memory::types::Visibility;

use super::AgentFilesState;
use crate::file_server::{FileRegistry, SentAttachment};

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
/// belong to the model's conversation, never to a client. The status line
/// the agent reads with a user message is left out the same way: nobody
/// wrote it.
#[derive(Debug, Serialize)]
pub struct HistoryMessage {
    /// The message, without its reasoning blocks.
    #[serde(flatten)]
    pub message: Message,
    /// The readable reasoning behind an assistant message, one entry per
    /// block that has text. Absent when the model produced none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub thinking: Vec<String>,
    /// When the message was recorded.
    #[serde(with = "residuum_core::time::minute_format")]
    pub timestamp: chrono::NaiveDateTime,
    /// Whether the message came from a user-visible or background turn.
    pub visibility: Visibility,
    /// The correlation id of the turn that produced the message, if known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    /// File attachment delivered to the web UI by a `send_message(endpoint = "ws")`
    /// tool call whose result this message records.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sent_attachment: Option<SentAttachment>,
}

impl From<RecentMessage> for HistoryMessage {
    fn from(recent: RecentMessage) -> Self {
        let thinking = readable_thinking(&recent.message.thinking)
            .into_iter()
            .map(str::to_string)
            .collect();
        let message = Message {
            thinking: Vec::new(),
            status_line: None,
            ..recent.message
        };
        Self {
            message,
            thinking,
            timestamp: recent.timestamp,
            visibility: recent.visibility,
            turn_id: recent.turn_id,
            sent_attachment: None,
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
/// Only the main conversation's episodes are history here: a session run's
/// episode (a pulse, a scheduled action, a spawned or external session)
/// shares the episodes directory but is never a cursor, and asking for one
/// by id is a 404.
///
/// With no query params, returns the live `Recent` segment plus a cursor
/// pointing at the newest main episode on disk (for the frontend's lazy-load).
///
/// With `?episode=ep-NNN`, returns that episode's transcript wrapped as
/// `RecentMessage`s plus a cursor to the next-older main episode. Returns
/// 404 when the episode does not exist or belongs to a session run.
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
            let recent = load_recent_messages(&recent_path).await.map_err(|err| {
                tracing::warn!(
                    error = %err,
                    path = %recent_path.display(),
                    "failed to load recent messages — refusing to silently return empty history",
                );
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
            let mut messages: Vec<HistoryMessage> =
                recent.into_iter().map(HistoryMessage::from).collect();
            refresh_attachment_paths(&state.workspace_dir, &mut messages).await;
            resolve_sent_attachments(&state.file_registry, &mut messages).await;
            let next_cursor = older_main_episode_id(&episodes_dir, None)
                .await
                .map_err(|err| {
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
            let path = residuum_memory::episode_store::find_episode_path(
                &episodes_dir,
                &episode_id,
            )
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
            if meta.source.is_session() {
                tracing::warn!(
                    episode = %episode_id,
                    session = meta.source.session_address.as_deref().unwrap_or_default(),
                    "refusing a session run's episode as main chat history",
                );
                return Err(StatusCode::NOT_FOUND);
            }

            let timestamp = meta.date.and_hms_opt(0, 0, 0).unwrap_or_default();
            let mut messages: Vec<HistoryMessage> = raw_messages
                .into_iter()
                .map(|message| HistoryMessage::from(wrap_episode_message(message, timestamp)))
                .collect();
            refresh_attachment_paths(&state.workspace_dir, &mut messages).await;
            resolve_sent_attachments(&state.file_registry, &mut messages).await;

            let next_cursor = older_main_episode_id(&episodes_dir, Some(&meta.id))
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
/// every model call (see `residuum_agent::usage::MainUsageSink`), the same
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

/// Point each attachment at its archived file when the file has moved.
///
/// Archiving an inbox item moves its attachment directory into the archive, so
/// an attachment recorded at `inbox/agent/attachments/<id>/<name>` is then only
/// at `archive/inbox/agent/attachments/<id>/<name>`. An attachment whose file is
/// still at its recorded path, or is in neither place, keeps the recorded path.
async fn refresh_attachment_paths(workspace_dir: &Path, messages: &mut [HistoryMessage]) {
    let archive_root = WorkspaceLayout::new(workspace_dir).agent_inbox_archive_attachments_dir();
    for history in messages {
        for attachment in &mut history.message.attachments {
            if file_exists(&workspace_dir.join(&attachment.path)).await {
                continue;
            }
            let archived = archive_root
                .join(&attachment.item_id)
                .join(&attachment.filename);
            if !file_exists(&archived).await {
                continue;
            }
            let Ok(relative) = archived.strip_prefix(workspace_dir) else {
                continue;
            };
            attachment.path = relative
                .components()
                .map(|part| part.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
        }
    }
}

/// Attach [`SentAttachment`] metadata to any `tool` message that records a
/// successful `send_message(endpoint = "ws", file_path = ...)` call.
async fn resolve_sent_attachments(registry: &FileRegistry, messages: &mut [HistoryMessage]) {
    let mut pending_ws_files: std::collections::HashMap<String, (String, Option<String>)> =
        std::collections::HashMap::new();
    for history in messages {
        match history.message.role {
            Role::Assistant => {
                for tc in history.message.tool_calls.iter().flatten() {
                    if let Some(spec) = parse_ws_file_send(&tc.name, &tc.arguments) {
                        pending_ws_files.insert(tc.id.clone(), spec);
                    }
                }
            }
            Role::Tool => {
                let Some(call_id) = &history.message.tool_call_id else {
                    continue;
                };
                let Some((file_path, caption)) = pending_ws_files.remove(call_id) else {
                    continue;
                };
                if is_ws_publish_success(&history.message.content) {
                    history.sent_attachment = registry
                        .resolve_sent_attachment(Path::new(&file_path), caption)
                        .await;
                }
            }
            Role::User | Role::System => {}
        }
    }
}

/// Extract `(file_path, caption)` from a `send_message` call targeting the
/// web UI (`endpoint = "ws"`) with a `file_path`.
fn parse_ws_file_send(
    name: &str,
    arguments: &serde_json::Value,
) -> Option<(String, Option<String>)> {
    if name != "send_message" {
        return None;
    }
    let endpoint = arguments
        .get("endpoint")
        .and_then(serde_json::Value::as_str)?;
    if endpoint != residuum_config::WEB_UI_ENDPOINT {
        return None;
    }
    if arguments
        .get("conversation")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|c| !c.is_empty())
    {
        return None;
    }
    let file_path = arguments
        .get("file_path")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())?;
    let caption = arguments
        .get("message")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    Some((file_path.to_string(), caption))
}

/// Whether `content` is the success output of a `send_message` call to `"ws"`.
fn is_ws_publish_success(content: &str) -> bool {
    content
        .lines()
        .next()
        .is_some_and(|line| line.ends_with(" published to endpoint 'ws'"))
}

/// Whether `path` exists as a file. A failed lookup is logged and reads as absent.
async fn file_exists(path: &Path) -> bool {
    match tokio::fs::try_exists(path).await {
        Ok(exists) => exists,
        Err(err) => {
            tracing::warn!(
                error = %err,
                path = %path.display(),
                "failed to check an attachment's location",
            );
            false
        }
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
    use residuum_inference::{MessageAttachment, ThinkingBlock};
    use residuum_memory::recent_messages::append_recent_messages;

    /// An assistant message whose reasoning the provider needs replayed:
    /// readable text, a signature to send back, and a block that is only
    /// encrypted.
    fn assistant_with_reasoning() -> Message {
        Message::assistant("The answer is 42.".to_string(), None).with_thinking(vec![
            ThinkingBlock {
                text: "First, the question.".to_string(),
                signature: Some("sig-first-secret".to_string()),
                redacted: None,
                part: None,
                origin: None,
                binding: None,
            },
            ThinkingBlock {
                text: String::new(),
                signature: None,
                redacted: Some("encrypted-secret".to_string()),
                part: None,
                origin: None,
                binding: None,
            },
            ThinkingBlock {
                text: "Then the arithmetic.".to_string(),
                signature: Some("sig-second-secret".to_string()),
                redacted: None,
                part: None,
                origin: None,
                binding: None,
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
        assert!(
            history.message.thinking.is_empty(),
            "expected {:?} to be empty",
            history.message.thinking
        );
    }

    /// A user message that came with one saved file, recorded at `path`.
    fn message_with_attachment(path: &str) -> HistoryMessage {
        let mut message = Message::user("see attached");
        message.attachments = vec![MessageAttachment {
            item_id: "item-1".to_string(),
            filename: "notes.md".to_string(),
            size: 7,
            content_type: "text/markdown".to_string(),
            path: path.to_string(),
        }];
        HistoryMessage::from(RecentMessage {
            message,
            timestamp: chrono::NaiveDateTime::default(),
            visibility: Visibility::User,
            turn_id: None,
        })
    }

    #[tokio::test]
    async fn archived_attachment_path_points_at_the_archived_file() {
        let dir = tempfile::tempdir().unwrap();
        let archived = dir
            .path()
            .join("archive/inbox/agent/attachments/item-1/notes.md");
        tokio::fs::create_dir_all(archived.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&archived, b"# notes").await.unwrap();
        let mut messages = vec![message_with_attachment(
            "inbox/agent/attachments/item-1/notes.md",
        )];

        refresh_attachment_paths(dir.path(), &mut messages).await;

        assert_eq!(
            messages[0].message.attachments[0].path,
            "archive/inbox/agent/attachments/item-1/notes.md",
            "an archived item's attachment should point at its archived file"
        );
    }

    #[tokio::test]
    async fn attachment_still_in_the_inbox_keeps_its_path() {
        let dir = tempfile::tempdir().unwrap();
        let active = dir.path().join("inbox/agent/attachments/item-1/notes.md");
        tokio::fs::create_dir_all(active.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&active, b"# notes").await.unwrap();
        let mut messages = vec![message_with_attachment(
            "inbox/agent/attachments/item-1/notes.md",
        )];

        refresh_attachment_paths(dir.path(), &mut messages).await;

        assert_eq!(
            messages[0].message.attachments[0].path,
            "inbox/agent/attachments/item-1/notes.md",
        );
    }

    #[tokio::test]
    async fn attachment_missing_everywhere_keeps_its_recorded_path() {
        let dir = tempfile::tempdir().unwrap();
        let mut messages = vec![message_with_attachment(
            "inbox/agent/attachments/item-1/notes.md",
        )];

        refresh_attachment_paths(dir.path(), &mut messages).await;

        assert_eq!(
            messages[0].message.attachments[0].path, "inbox/agent/attachments/item-1/notes.md",
            "a file found nowhere keeps the path it was recorded with"
        );
    }

    #[tokio::test]
    async fn history_attaches_durable_workspace_url_for_successful_ws_file_send() {
        let dir = tempfile::tempdir().unwrap();
        let outbox = dir.path().join("outbox");
        tokio::fs::create_dir_all(&outbox).await.unwrap();
        let file = outbox.join("chart.png");
        tokio::fs::write(&file, b"png-bytes").await.unwrap();

        let call = residuum_inference::ToolCall {
            id: "call-1".to_string(),
            name: "send_message".to_string(),
            arguments: serde_json::json!({
                "endpoint": "ws",
                "file_path": file.to_string_lossy(),
                "message": "Monthly chart"
            }),
            server: None,
        };
        append_recent_messages(
            &dir.path().join("recent_messages.json"),
            &[
                Message::assistant(String::new(), Some(vec![call])),
                Message::tool(
                    "Message and file 'chart.png' published to endpoint 'ws'",
                    "call-1",
                ),
            ],
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
        assert_eq!(
            messages[1].get("sent_attachment"),
            Some(&serde_json::json!({
                "filename": "chart.png",
                "mime_type": "image/png",
                "size": 9,
                "url": "/api/agents/test-agent/files/workspace?path=outbox%2Fchart.png",
                "caption": "Monthly chart"
            }))
        );
    }

    #[tokio::test]
    async fn history_omits_sent_attachment_when_send_message_failed_or_file_is_gone() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("chart.png");
        tokio::fs::write(&file, b"png-bytes").await.unwrap();

        let failed_call = residuum_inference::ToolCall {
            id: "call-err".to_string(),
            name: "send_message".to_string(),
            arguments: serde_json::json!({
                "endpoint": "ws",
                "file_path": file.to_string_lossy(),
            }),
            server: None,
        };
        let missing_call = residuum_inference::ToolCall {
            id: "call-gone".to_string(),
            name: "send_message".to_string(),
            arguments: serde_json::json!({
                "endpoint": "ws",
                "file_path": dir.path().join("deleted.png").to_string_lossy(),
            }),
            server: None,
        };
        append_recent_messages(
            &dir.path().join("recent_messages.json"),
            &[
                Message::assistant(String::new(), Some(vec![failed_call, missing_call])),
                Message::tool("sessions cannot message the owner directly", "call-err"),
                Message::tool("File 'deleted.png' published to endpoint 'ws'", "call-gone"),
            ],
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
        assert!(messages[1].get("sent_attachment").is_none());
        assert!(messages[2].get("sent_attachment").is_none());
    }
}
