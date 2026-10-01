//! What an agent's last message was, read from its files: how the overview
//! answers for an agent that isn't running, and for one that just started
//! and hasn't had a turn yet.

use std::path::Path;

use anyhow::Context as _;
use chrono::NaiveTime;
use tokio::io::{AsyncBufReadExt as _, BufReader};

use super::preview::plain_preview;
use super::types::{LastMessage, LastMessageRole, TimePrecision};
use crate::hub::AgentFiles;
use crate::inference::{Message, Role};
use crate::memory::episode_store::{EpisodeMeta, read_episode_jsonl, transcripts_newest_first};
use crate::memory::recent_messages::load_recent_messages;
use crate::memory::types::Visibility;
use crate::time::local_to_rfc3339;
use crate::workspace::layout::WorkspaceLayout;

/// The newest message of the agent's main conversation that the user saw: in
/// its recent history, else in the newest episode of the main conversation
/// that has one, which dates it to the day.
///
/// An unreadable file is logged and read past, so one broken file costs the
/// preview and not the overview.
pub(super) async fn last_message(agent: &str, files: &AgentFiles) -> Option<LastMessage> {
    let layout = WorkspaceLayout::new(&files.dir);
    let path = layout.recent_messages_json();
    match load_recent_messages(&path).await {
        Ok(recent) => {
            let found = recent
                .iter()
                .rev()
                .filter(|recent| recent.visibility == Visibility::User)
                .find_map(|recent| {
                    let (role, preview) = shown(&recent.message)?;
                    Some(LastMessage {
                        role,
                        preview,
                        at: local_to_rfc3339(files.timezone, recent.timestamp),
                        at_precision: TimePrecision::Minute,
                    })
                });
            if found.is_some() {
                return found;
            }
        }
        Err(e) => {
            tracing::warn!(agent = %agent, path = %path.display(), error = %format!("{e:#}"), "couldn't read an agent's recent messages for its overview; looking in its episodes instead");
        }
    }
    last_message_in_episodes(agent, files, &layout).await
}

/// Who wrote `message` and what it says, when it is something the user saw
/// the agent or themselves say: a user or assistant message with text.
fn shown(message: &Message) -> Option<(LastMessageRole, String)> {
    let role = match message.role {
        Role::User => LastMessageRole::User,
        Role::Assistant => LastMessageRole::Assistant,
        Role::System | Role::Tool => return None,
    };
    let preview = plain_preview(&message.content);
    (!preview.is_empty()).then_some((role, preview))
}

/// The newest message of the newest main-conversation episode that has one.
/// Episodes keep the date and not the time, and not who saw each message, so
/// every message counts and the time is the start of the day.
async fn last_message_in_episodes(
    agent: &str,
    files: &AgentFiles,
    layout: &WorkspaceLayout,
) -> Option<LastMessage> {
    let dir = layout.episodes_dir();
    let listed = crate::util::spawn_blocking_in_span({
        let dir = dir.clone();
        move || transcripts_newest_first(&dir)
    })
    .await;
    let transcripts = match listed {
        Ok(Ok(transcripts)) => transcripts,
        Ok(Err(e)) => {
            tracing::warn!(agent = %agent, path = %dir.display(), error = %format!("{e:#}"), "couldn't list an agent's episodes for its overview");
            return None;
        }
        Err(e) => {
            tracing::warn!(agent = %agent, path = %dir.display(), error = %e, "listing an agent's episodes for its overview ended abnormally");
            return None;
        }
    };
    for transcript in transcripts {
        match main_conversation_message(&transcript).await {
            Ok(Some((meta, role, preview))) => {
                return Some(LastMessage {
                    role,
                    preview,
                    at: local_to_rfc3339(files.timezone, meta.date.and_time(NaiveTime::MIN)),
                    at_precision: TimePrecision::Day,
                });
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(agent = %agent, path = %transcript.display(), error = %format!("{e:#}"), "couldn't read an episode for an agent's overview; trying an older one");
            }
        }
    }
    None
}

/// The episode's newest message that is shown, when the episode is of the
/// main conversation. A session's episode isn't, and is told by its first
/// line alone so it costs a line to skip.
async fn main_conversation_message(
    transcript: &Path,
) -> anyhow::Result<Option<(EpisodeMeta, LastMessageRole, String)>> {
    let file = tokio::fs::File::open(transcript)
        .await
        .with_context(|| format!("failed to open episode at {}", transcript.display()))?;
    let mut first_line = String::new();
    BufReader::new(file)
        .read_line(&mut first_line)
        .await
        .with_context(|| format!("failed to read episode at {}", transcript.display()))?;
    let meta: EpisodeMeta = serde_json::from_str(&first_line)
        .with_context(|| format!("failed to parse episode meta at {}", transcript.display()))?;
    if meta.source.is_session() {
        return Ok(None);
    }
    let (_, messages) = read_episode_jsonl(transcript).await?;
    Ok(messages
        .iter()
        .rev()
        .find_map(shown)
        .map(|(role, preview)| (meta, role, preview)))
}
