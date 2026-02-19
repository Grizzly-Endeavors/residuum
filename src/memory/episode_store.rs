//! Episode transcript file persistence.
//!
//! Writes episode transcripts as markdown files with hand-written YAML
//! frontmatter to `memory/episodes/YYYY-MM/<id>.md`.

use std::path::Path;

use crate::error::IronclawError;
use crate::memory::types::Episode;
use crate::models::Message;

/// Write an episode transcript file to the episodes directory.
///
/// Creates `{episodes_dir}/{YYYY-MM}/{episode.id}.md` with YAML frontmatter
/// followed by the formatted conversation transcript. Creates the month
/// subdirectory if it doesn't exist.
///
/// # Errors
/// Returns an error if the file cannot be written.
pub async fn write_episode_transcript(
    episodes_dir: &Path,
    episode: &Episode,
    messages: &[Message],
) -> Result<(), IronclawError> {
    let month_dir = episodes_dir.join(episode.date.format("%Y-%m").to_string());
    tokio::fs::create_dir_all(&month_dir).await.map_err(|e| {
        IronclawError::Memory(format!(
            "failed to create episode directory at {}: {e}",
            month_dir.display()
        ))
    })?;

    let filename = format!("{}.md", episode.id);
    let path = month_dir.join(&filename);

    let frontmatter = format_frontmatter(episode);
    let transcript = format_transcript(messages);
    let content = format!("{frontmatter}\n{transcript}");

    tokio::fs::write(&path, &content).await.map_err(|e| {
        IronclawError::Memory(format!(
            "failed to write episode transcript at {}: {e}",
            path.display()
        ))
    })
}

/// Get the path where an episode transcript would be written.
#[must_use]
pub fn episode_path(episodes_dir: &Path, episode: &Episode) -> std::path::PathBuf {
    episodes_dir
        .join(episode.date.format("%Y-%m").to_string())
        .join(format!("{}.md", episode.id))
}

/// Format YAML frontmatter for an episode.
///
/// Hand-written to avoid a YAML serialization dependency.
#[must_use]
pub fn format_frontmatter(episode: &Episode) -> String {
    let mut parts = Vec::with_capacity(8);
    parts.push("---".to_string());
    parts.push(format!("id: {}", episode.id));
    parts.push(format!("date: {}", episode.date));
    parts.push(format!("start: {}", episode.start));
    parts.push(format!("end: {}", episode.end));
    parts.push(format!("context: {}", episode.context));
    parts.push("---".to_string());
    parts.join("\n")
}

/// Format messages as a readable markdown transcript.
#[must_use]
pub fn format_transcript(messages: &[Message]) -> String {
    let parts: Vec<String> = messages.iter().map(format_single_message).collect();
    parts.join("\n")
}

/// Format a single message as a markdown block.
fn format_single_message(msg: &Message) -> String {
    let role_label = match msg.role {
        crate::models::Role::System => "**System**",
        crate::models::Role::User => "**User**",
        crate::models::Role::Assistant => "**Assistant**",
        crate::models::Role::Tool => "**Tool**",
    };

    let header = match &msg.tool_call_id {
        Some(id) => format!("{role_label} (call: {id})\n"),
        None => format!("{role_label}\n"),
    };

    let content = if msg.content.is_empty() {
        String::new()
    } else {
        format!("{}\n", msg.content)
    };

    let tool_calls: Vec<String> = msg
        .tool_calls
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|call| {
            format!(
                "\n> Tool call: `{}` (id: {})\n> ```json\n> {}\n> ```\n",
                call.name, call.id, call.arguments
            )
        })
        .collect();

    format!("{header}{content}{}", tool_calls.join(""))
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use crate::models::{Role, ToolCall};
    use chrono::NaiveDate;

    fn sample_episode() -> Episode {
        Episode {
            id: "ep-001".to_string(),
            date: NaiveDate::from_ymd_opt(2026, 2, 19).unwrap(),
            start: "user asked about files".to_string(),
            end: "listed directory contents".to_string(),
            context: "general".to_string(),
            observations: vec!["user prefers concise output".to_string()],
            source_episodes: vec![],
        }
    }

    #[test]
    fn frontmatter_format() {
        let episode = sample_episode();
        let fm = format_frontmatter(&episode);

        assert!(fm.starts_with("---"), "should start with ---");
        assert!(fm.ends_with("---"), "should end with ---");
        assert!(fm.contains("id: ep-001"), "should contain id");
        assert!(fm.contains("date: 2026-02-19"), "should contain date");
        assert!(
            fm.contains("start: user asked about files"),
            "should contain start"
        );
        assert!(
            fm.contains("end: listed directory contents"),
            "should contain end"
        );
        assert!(fm.contains("context: general"), "should contain context");
    }

    #[test]
    fn transcript_basic_messages() {
        let messages = vec![
            Message {
                role: Role::User,
                content: "what is 2+2?".to_string(),
                tool_calls: None,
                tool_call_id: None,
            },
            Message {
                role: Role::Assistant,
                content: "2+2 equals 4.".to_string(),
                tool_calls: None,
                tool_call_id: None,
            },
        ];

        let transcript = format_transcript(&messages);
        assert!(transcript.contains("**User**"), "should have user label");
        assert!(
            transcript.contains("**Assistant**"),
            "should have assistant label"
        );
        assert!(
            transcript.contains("what is 2+2?"),
            "should have user content"
        );
        assert!(
            transcript.contains("2+2 equals 4."),
            "should have assistant content"
        );
    }

    #[test]
    fn transcript_with_tool_calls() {
        let messages = vec![Message {
            role: Role::Assistant,
            content: String::new(),
            tool_calls: Some(vec![ToolCall {
                id: "call_1".to_string(),
                name: "exec".to_string(),
                arguments: serde_json::json!({"command": "ls"}),
            }]),
            tool_call_id: None,
        }];

        let transcript = format_transcript(&messages);
        assert!(
            transcript.contains("Tool call: `exec`"),
            "should show tool call name"
        );
        assert!(transcript.contains("call_1"), "should show tool call ID");
    }

    #[test]
    fn transcript_with_tool_result() {
        let messages = vec![Message {
            role: Role::Tool,
            content: "file1.txt\nfile2.txt".to_string(),
            tool_calls: None,
            tool_call_id: Some("call_1".to_string()),
        }];

        let transcript = format_transcript(&messages);
        assert!(transcript.contains("**Tool**"), "should have tool label");
        assert!(
            transcript.contains("(call: call_1)"),
            "should show call ID reference"
        );
        assert!(
            transcript.contains("file1.txt"),
            "should contain tool output"
        );
    }

    #[tokio::test]
    async fn write_transcript_creates_file_in_month_dir() {
        let dir = tempfile::tempdir().unwrap();
        let episode = sample_episode();
        let messages = vec![Message {
            role: Role::User,
            content: "hello".to_string(),
            tool_calls: None,
            tool_call_id: None,
        }];

        write_episode_transcript(dir.path(), &episode, &messages)
            .await
            .unwrap();

        let path = dir.path().join("2026-02/ep-001.md");
        assert!(
            path.exists(),
            "transcript file should be created in month subdir"
        );

        let contents = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(contents.starts_with("---"), "should start with frontmatter");
        assert!(contents.contains("hello"), "should contain message content");
    }

    #[test]
    fn episode_path_includes_month() {
        let episode = sample_episode();
        let path = episode_path(std::path::Path::new("/ws/episodes"), &episode);
        assert_eq!(
            path,
            std::path::PathBuf::from("/ws/episodes/2026-02/ep-001.md"),
            "path should include YYYY-MM subdirectory"
        );
    }
}
