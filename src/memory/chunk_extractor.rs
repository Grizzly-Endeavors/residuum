//! Interaction-pair chunk extraction from episode transcripts.
//!
//! Extracts user-question + assistant-text-response pairs from messages,
//! producing `IndexChunk` values for granular BM25 indexing. Tool-call-only
//! assistant messages and tool/system messages are skipped.

use std::path::Path;

use crate::error::IronclawError;
use crate::memory::types::IndexChunk;
use crate::models::{Message, Role};

/// Extract interaction-pair chunks from a sequence of messages.
///
/// Walks messages in order. A chunk is closed when an assistant message with
/// non-empty text content follows a pending user message. Tool-only assistant
/// messages (empty/whitespace text content) are skipped. System and tool
/// messages are also skipped.
///
/// `line_offset` is the transcript line number of the first message (typically 2,
/// since line 1 is the meta object in JSONL transcripts).
#[must_use]
pub(crate) fn extract_chunks(
    messages: &[Message],
    episode_id: &str,
    date: &str,
    ctx: &str,
    line_offset: usize,
) -> Vec<IndexChunk> {
    let mut chunks = Vec::new();
    let mut pending_user: Option<(usize, &str)> = None; // (line_number, content)

    for (i, msg) in messages.iter().enumerate() {
        let line_num = line_offset + i;
        match msg.role {
            Role::User => {
                // New user message — set (or replace) pending
                pending_user = Some((line_num, &msg.content));
            }
            Role::Assistant => {
                let text = msg.content.trim();
                if text.is_empty() {
                    // Tool-call-only assistant message — skip, keep pending user
                    continue;
                }
                if let Some((user_line, user_content)) = pending_user.take() {
                    let chunk_id = format!("{episode_id}-c{}", chunks.len());
                    chunks.push(IndexChunk {
                        chunk_id,
                        episode_id: episode_id.to_string(),
                        date: date.to_string(),
                        context: ctx.to_string(),
                        line_start: user_line,
                        line_end: line_num,
                        content: format!("user: {user_content}\nassistant: {text}"),
                    });
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
pub(crate) async fn write_idx_jsonl(
    path: &Path,
    chunks: &[IndexChunk],
) -> Result<(), IronclawError> {
    let mut lines = Vec::with_capacity(chunks.len());
    for chunk in chunks {
        lines.push(
            serde_json::to_string(chunk).map_err(|e| {
                IronclawError::Memory(format!("failed to serialize index chunk: {e}"))
            })?,
        );
    }
    let content = if lines.is_empty() {
        String::new()
    } else {
        lines.join("\n") + "\n"
    };

    let dir = path.parent().ok_or_else(|| {
        IronclawError::Memory(format!(
            "idx.jsonl path has no parent directory: {}",
            path.display()
        ))
    })?;

    let tmp_path = dir.join(".idx.jsonl.tmp");
    tokio::fs::write(&tmp_path, &content).await.map_err(|e| {
        IronclawError::Memory(format!(
            "failed to write idx.jsonl at {}: {e}",
            tmp_path.display()
        ))
    })?;

    tokio::fs::rename(&tmp_path, path).await.map_err(|e| {
        IronclawError::Memory(format!(
            "failed to rename idx.jsonl from {} to {}: {e}",
            tmp_path.display(),
            path.display()
        ))
    })?;

    Ok(())
}

/// Read and parse an idx.jsonl file, returning chunks.
///
/// Returns an empty Vec for missing or empty files. Skips unparseable lines with a warning.
pub(crate) fn read_idx_jsonl(path: &Path) -> Vec<IndexChunk> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
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
    use crate::models::ToolCall;

    fn user(text: &str) -> Message {
        Message::user(text)
    }

    fn assistant(text: &str) -> Message {
        Message::assistant(text.to_string(), None)
    }

    fn assistant_tool_only() -> Message {
        Message::assistant(
            String::new(),
            Some(vec![ToolCall {
                id: "call_1".to_string(),
                name: "read_file".to_string(),
                arguments: serde_json::json!({"path": "src/main.rs"}),
            }]),
        )
    }

    fn tool_result() -> Message {
        Message::tool("file contents here", "call_1")
    }

    fn system_msg() -> Message {
        Message::system("you are a helpful assistant")
    }

    #[test]
    fn basic_pairing() {
        let msgs = vec![user("how does X work?"), assistant("X works by doing Y.")];
        let chunks = extract_chunks(&msgs, "ep-001", "2026-02-19", "ironclaw", 2);
        assert_eq!(chunks.len(), 1, "should produce one chunk");
        assert_eq!(chunks[0].chunk_id, "ep-001-c0");
        assert!(chunks[0].content.contains("user: how does X work?"));
        assert!(chunks[0].content.contains("assistant: X works by doing Y."));
    }

    #[test]
    fn tool_only_assistant_skipped() {
        let msgs = vec![
            user("read a file"),
            assistant_tool_only(),
            tool_result(),
            assistant("here are the file contents"),
        ];
        let chunks = extract_chunks(&msgs, "ep-001", "2026-02-19", "ironclaw", 2);
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
        // User message followed by another user message — first is discarded
        let msgs = vec![
            user("first question"),
            user("second question"),
            assistant("answer to second"),
        ];
        let chunks = extract_chunks(&msgs, "ep-001", "2026-02-19", "ironclaw", 2);
        assert_eq!(chunks.len(), 1, "first user message should be discarded");
        assert!(chunks[0].content.contains("user: second question"));
    }

    #[test]
    fn empty_content_skipped() {
        let msgs = vec![
            user("question"),
            Message::assistant("   ".to_string(), None), // whitespace-only
        ];
        let chunks = extract_chunks(&msgs, "ep-001", "2026-02-19", "ironclaw", 2);
        assert!(
            chunks.is_empty(),
            "whitespace-only assistant should not close pair"
        );
    }

    #[test]
    fn line_numbers_correct() {
        let msgs = vec![
            user("q1"),      // line 2
            assistant("a1"), // line 3
            user("q2"),      // line 4
            assistant("a2"), // line 5
        ];
        let chunks = extract_chunks(&msgs, "ep-001", "2026-02-19", "ironclaw", 2);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].line_start, 2);
        assert_eq!(chunks[0].line_end, 3);
        assert_eq!(chunks[1].line_start, 4);
        assert_eq!(chunks[1].line_end, 5);
    }

    #[test]
    fn no_messages() {
        let chunks = extract_chunks(&[], "ep-001", "2026-02-19", "ironclaw", 2);
        assert!(chunks.is_empty(), "no messages should produce no chunks");
    }

    #[test]
    fn all_tool_messages() {
        let msgs = vec![tool_result(), system_msg(), tool_result()];
        let chunks = extract_chunks(&msgs, "ep-001", "2026-02-19", "ironclaw", 2);
        assert!(
            chunks.is_empty(),
            "tool-only messages should produce no chunks"
        );
    }

    #[test]
    fn chunk_ids_sequential() {
        let msgs = vec![
            user("q1"),
            assistant("a1"),
            user("q2"),
            assistant("a2"),
            user("q3"),
            assistant("a3"),
        ];
        let chunks = extract_chunks(&msgs, "ep-001", "2026-02-19", "ironclaw", 2);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].chunk_id, "ep-001-c0");
        assert_eq!(chunks[1].chunk_id, "ep-001-c1");
        assert_eq!(chunks[2].chunk_id, "ep-001-c2");
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
                context: "ironclaw".to_string(),
                line_start: 2,
                line_end: 3,
                content: "user: hello\nassistant: hi".to_string(),
            },
            IndexChunk {
                chunk_id: "ep-001-c1".to_string(),
                episode_id: "ep-001".to_string(),
                date: "2026-02-19".to_string(),
                context: "ironclaw".to_string(),
                line_start: 4,
                line_end: 5,
                content: "user: what\nassistant: that".to_string(),
            },
        ];

        write_idx_jsonl(&path, &chunks).await.unwrap();
        let loaded = read_idx_jsonl(&path);

        assert_eq!(loaded.len(), 2, "should round-trip 2 chunks");
        assert_eq!(loaded[0].chunk_id, "ep-001-c0");
        assert_eq!(loaded[1].chunk_id, "ep-001-c1");
        assert_eq!(loaded[0].content, "user: hello\nassistant: hi");
    }
}
