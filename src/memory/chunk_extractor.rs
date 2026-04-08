//! Interaction-pair chunk extraction from episode transcripts.
//!
//! Extracts user-question + assistant-text-response pairs from messages,
//! producing `IndexChunk` values for granular BM25 indexing. Tool-call-only
//! assistant messages and tool/system messages are skipped.

use std::path::Path;

use anyhow::Context;

use crate::memory::recent_messages::RecentMessage;
use crate::memory::types::IndexChunk;
use crate::models::Role;

/// Extract interaction-pair chunks from a sequence of recent messages.
///
/// Walks messages in order. A chunk is closed when an assistant message with
/// non-empty text content follows a pending user message. Tool-only assistant
/// messages (empty/whitespace text content) are skipped. System and tool
/// messages are also skipped.
///
/// Each chunk inherits `project_context` from the user `RecentMessage` that
/// started the pair.
///
/// `line_offset` is the transcript line number of the first message (typically 2,
/// since line 1 is the meta object in JSONL transcripts).
#[must_use]
pub(crate) fn extract_chunks(
    recent_messages: &[RecentMessage],
    episode_id: &str,
    date: &str,
    line_offset: usize,
) -> Vec<IndexChunk> {
    let mut chunks = Vec::new();
    // (line_number, content, project_context)
    let mut pending_user: Option<(usize, &str, &str)> = None;

    for (i, rm) in recent_messages.iter().enumerate() {
        let msg = &rm.message;
        let line_num = line_offset + i;
        match msg.role {
            Role::User => {
                // New user message — set (or replace) pending
                pending_user = Some((line_num, &msg.content, &rm.project_context));
            }
            Role::Assistant => {
                let text = msg.content.trim();
                if text.is_empty() {
                    // Tool-call-only assistant message — skip, keep pending user
                    continue;
                }
                if let Some((user_line, user_content, user_ctx)) = pending_user.take() {
                    let chunk_id = format!("{episode_id}-c{}", chunks.len());
                    chunks.push(IndexChunk {
                        chunk_id,
                        episode_id: episode_id.to_string(),
                        date: date.to_string(),
                        context: user_ctx.to_string(),
                        line_start: user_line,
                        line_end: line_num,
                        content: format!("user: {user_content}\nassistant: {text}"),
                    });
                } else {
                    tracing::debug!(
                        episode_id,
                        line = line_num,
                        "orphaned assistant message with no pending user"
                    );
                }
                // If no pending user, this assistant message is orphaned — skip
            }
            Role::Tool | Role::System => {
                // Skip tool results and system messages entirely
            }
        }
    }

    chunks
}

/// Write chunks to an idx.jsonl file atomically (temp file + rename).
///
/// Each line is a JSON-serialized `IndexChunk`.
///
/// # Errors
/// Returns an error if the file cannot be written.
pub(crate) async fn write_idx_jsonl(path: &Path, chunks: &[IndexChunk]) -> anyhow::Result<()> {
    let mut lines = Vec::with_capacity(chunks.len());
    for chunk in chunks {
        lines.push(serde_json::to_string(chunk).context("failed to serialize index chunk")?);
    }
    let content = if lines.is_empty() {
        String::new()
    } else {
        lines.join("\n") + "\n"
    };

    crate::util::fs::atomic_write(path, &content).await
}

/// Read and parse an idx.jsonl file, returning chunks.
///
/// Returns an empty Vec for missing or empty files. Skips unparseable lines with a warning.
pub(crate) fn read_idx_jsonl(path: &Path) -> Vec<IndexChunk> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "failed to read idx.jsonl");
            return Vec::new();
        }
    };

    let mut chunks = Vec::new();
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<IndexChunk>(line) {
            Ok(chunk) => chunks.push(chunk),
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "skipping unparseable idx.jsonl line"
                );
            }
        }
    }
    chunks
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
#[expect(
    clippy::indexing_slicing,
    reason = "test code indexes into known-length slices"
)]
mod tests {
    use super::*;
    use crate::memory::types::Visibility;
    use crate::models::{Message, ToolCall};

    fn recent_user(text: &str) -> RecentMessage {
        RecentMessage {
            message: Message::user(text),
            timestamp: chrono::Utc::now().naive_utc(),
            project_context: "residuum".to_string(),
            visibility: Visibility::User,
        }
    }

    fn recent_assistant(text: &str) -> RecentMessage {
        RecentMessage {
            message: Message::assistant(text.to_string(), None),
            timestamp: chrono::Utc::now().naive_utc(),
            project_context: "residuum".to_string(),
            visibility: Visibility::User,
        }
    }

    fn recent_assistant_tool_only() -> RecentMessage {
        RecentMessage {
            message: Message::assistant(
                String::new(),
                Some(vec![ToolCall {
                    id: "call_1".to_string(),
                    name: "read_file".to_string(),
                    arguments: serde_json::json!({"path": "src/main.rs"}),
                }]),
            ),
            timestamp: chrono::Utc::now().naive_utc(),
            project_context: "residuum".to_string(),
            visibility: Visibility::User,
        }
    }

    fn recent_tool() -> RecentMessage {
        RecentMessage {
            message: Message::tool("file contents here", "call_1"),
            timestamp: chrono::Utc::now().naive_utc(),
            project_context: "residuum".to_string(),
            visibility: Visibility::User,
        }
    }

    fn recent_system() -> RecentMessage {
        RecentMessage {
            message: Message::system("you are a helpful assistant"),
            timestamp: chrono::Utc::now().naive_utc(),
            project_context: "residuum".to_string(),
            visibility: Visibility::User,
        }
    }

    #[test]
    fn basic_pairing() {
        let msgs = vec![
            recent_user("how does X work?"),
            recent_assistant("X works by doing Y."),
        ];
        let chunks = extract_chunks(&msgs, "ep-001", "2026-02-19", 2);
        assert_eq!(chunks.len(), 1, "should produce one chunk");
        assert_eq!(chunks[0].chunk_id, "ep-001-c0");
        assert!(chunks[0].content.contains("user: how does X work?"));
        assert!(chunks[0].content.contains("assistant: X works by doing Y."));
    }

    #[test]
    fn tool_only_assistant_skipped() {
        let msgs = vec![
            recent_user("read a file"),
            recent_assistant_tool_only(),
            recent_tool(),
            recent_assistant("here are the file contents"),
        ];
        let chunks = extract_chunks(&msgs, "ep-001", "2026-02-19", 2);
        assert_eq!(
            chunks.len(),
            1,
            "should produce one chunk (skipping tool-only)"
        );
        assert!(chunks[0].content.contains("user: read a file"));
        assert!(
            chunks[0]
                .content
                .contains("assistant: here are the file contents")
        );
    }

    #[test]
    fn incomplete_pair_discarded() {
        let msgs = vec![
            recent_user("first question"),
            recent_user("second question"),
            recent_assistant("answer to second"),
        ];
        let chunks = extract_chunks(&msgs, "ep-001", "2026-02-19", 2);
        assert_eq!(chunks.len(), 1, "first user message should be discarded");
        assert!(chunks[0].content.contains("user: second question"));
    }

    #[test]
    fn empty_content_skipped() {
        let msgs = vec![
            recent_user("question"),
            RecentMessage {
                message: Message::assistant("   ".to_string(), None),
                timestamp: chrono::Utc::now().naive_utc(),
                project_context: "residuum".to_string(),
                visibility: Visibility::User,
            },
        ];
        let chunks = extract_chunks(&msgs, "ep-001", "2026-02-19", 2);
        assert!(
            chunks.is_empty(),
            "whitespace-only assistant should not close pair"
        );
    }

    #[test]
    fn line_numbers_correct() {
        let msgs = vec![
            recent_user("q1"),      // line 2
            recent_assistant("a1"), // line 3
            recent_user("q2"),      // line 4
            recent_assistant("a2"), // line 5
        ];
        let chunks = extract_chunks(&msgs, "ep-001", "2026-02-19", 2);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].line_start, 2);
        assert_eq!(chunks[0].line_end, 3);
        assert_eq!(chunks[1].line_start, 4);
        assert_eq!(chunks[1].line_end, 5);
    }

    #[test]
    fn no_messages() {
        let chunks = extract_chunks(&[], "ep-001", "2026-02-19", 2);
        assert!(chunks.is_empty(), "no messages should produce no chunks");
    }

    #[test]
    fn all_tool_messages() {
        let msgs = vec![recent_tool(), recent_system(), recent_tool()];
        let chunks = extract_chunks(&msgs, "ep-001", "2026-02-19", 2);
        assert!(
            chunks.is_empty(),
            "tool-only messages should produce no chunks"
        );
    }

    #[test]
    fn chunk_ids_sequential() {
        let msgs = vec![
            recent_user("q1"),
            recent_assistant("a1"),
            recent_user("q2"),
            recent_assistant("a2"),
            recent_user("q3"),
            recent_assistant("a3"),
        ];
        let chunks = extract_chunks(&msgs, "ep-001", "2026-02-19", 2);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].chunk_id, "ep-001-c0");
        assert_eq!(chunks[1].chunk_id, "ep-001-c1");
        assert_eq!(chunks[2].chunk_id, "ep-001-c2");
    }

    #[test]
    fn per_chunk_project_context() {
        let msgs = vec![
            RecentMessage {
                message: Message::user("how is residuum?"),
                timestamp: chrono::Utc::now().naive_utc(),
                project_context: "residuum".to_string(),
                visibility: Visibility::User,
            },
            recent_assistant("it's great"),
            RecentMessage {
                message: Message::user("how about devops?"),
                timestamp: chrono::Utc::now().naive_utc(),
                project_context: "devops".to_string(),
                visibility: Visibility::User,
            },
            recent_assistant("also good"),
        ];
        let chunks = extract_chunks(&msgs, "ep-001", "2026-02-19", 2);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].context, "residuum");
        assert_eq!(chunks[1].context, "devops");
    }

    #[tokio::test]
    async fn write_and_read_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ep-001.idx.jsonl");

        let chunks = vec![
            IndexChunk {
                chunk_id: "ep-001-c0".to_string(),
                episode_id: "ep-001".to_string(),
                date: "2026-02-19".to_string(),
                context: "residuum".to_string(),
                line_start: 2,
                line_end: 3,
                content: "user: hello\nassistant: hi".to_string(),
            },
            IndexChunk {
                chunk_id: "ep-001-c1".to_string(),
                episode_id: "ep-001".to_string(),
                date: "2026-02-19".to_string(),
                context: "residuum".to_string(),
                line_start: 4,
                line_end: 5,
                content: "user: what\nassistant: that".to_string(),
            },
        ];

        write_idx_jsonl(&path, &chunks).await.unwrap();
        let loaded = read_idx_jsonl(&path);

        assert_eq!(loaded.len(), 2, "should round-trip 2 chunks");
        assert_eq!(loaded[0].chunk_id, "ep-001-c0");
        assert_eq!(loaded[0].episode_id, "ep-001");
        assert_eq!(loaded[0].date, "2026-02-19");
        assert_eq!(loaded[0].context, "residuum");
        assert_eq!(loaded[0].line_start, 2);
        assert_eq!(loaded[0].line_end, 3);
        assert_eq!(loaded[0].content, "user: hello\nassistant: hi");
        assert_eq!(loaded[1].chunk_id, "ep-001-c1");
        assert_eq!(loaded[1].content, "user: what\nassistant: that");
    }

    #[test]
    fn read_idx_jsonl_missing_file_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.idx.jsonl");
        let chunks = read_idx_jsonl(&path);
        assert!(chunks.is_empty(), "missing file should return empty vec");
    }

    #[tokio::test]
    async fn read_idx_jsonl_skips_malformed_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ep-001.idx.jsonl");

        let valid = IndexChunk {
            chunk_id: "ep-001-c0".to_string(),
            episode_id: "ep-001".to_string(),
            date: "2026-02-19".to_string(),
            context: "residuum".to_string(),
            line_start: 2,
            line_end: 3,
            content: "user: hello\nassistant: hi".to_string(),
        };
        let valid2 = IndexChunk {
            chunk_id: "ep-001-c1".to_string(),
            ..valid.clone()
        };
        let content = format!(
            "{}\nnot valid json\n{}\n",
            serde_json::to_string(&valid).unwrap(),
            serde_json::to_string(&valid2).unwrap(),
        );
        tokio::fs::write(&path, &content).await.unwrap();

        let chunks = read_idx_jsonl(&path);
        assert_eq!(
            chunks.len(),
            2,
            "should skip malformed line and return 2 valid chunks"
        );
        assert_eq!(chunks[0].chunk_id, "ep-001-c0");
        assert_eq!(chunks[1].chunk_id, "ep-001-c1");
    }
}
