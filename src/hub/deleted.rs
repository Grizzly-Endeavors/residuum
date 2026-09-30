//! The record of an agent's deletion.
//!
//! A deleted agent's checkpoint repositories stay under
//! `hub/checkpoints/agents/<name>/`. Their newest checkpoint is often much
//! older than the deletion (deleting an unchanged workspace reuses the
//! existing tip), a turn-end checkpoint still in flight when the agent stops
//! can land after it, and none of them holds the agent's role page, which
//! lives in the team folder. So the deletion writes `deleted.json` beside the
//! repositories holding the time, the workspace checkpoint taken for the
//! deletion, and the role page's text. Creating or restoring the agent again
//! removes it.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::checkpoints::agent_repos_dir;

const FILE_NAME: &str = "deleted.json";

/// What a deletion leaves beside the agent's checkpoint repositories.
#[derive(Serialize, Deserialize)]
pub(super) struct DeletionRecord {
    /// When the agent was deleted.
    pub deleted_at: DateTime<Utc>,
    /// The workspace checkpoint taken of the agent's directory for the
    /// deletion, when one could be recorded.
    #[serde(default)]
    pub checkpoint_id: Option<String>,
    /// The agent's role page as it was, when it had one.
    #[serde(default)]
    pub role_page: Option<String>,
}

fn record_path(checkpoints_dir: &Path, name: &str) -> PathBuf {
    agent_repos_dir(checkpoints_dir, name).join(FILE_NAME)
}

/// Record that the agent `name` was deleted. A failure is logged and leaves
/// the listing to fall back to the newest checkpoint's time and a restore to
/// a placeholder role page.
pub(super) async fn record_deletion(checkpoints_dir: &Path, name: &str, record: &DeletionRecord) {
    let path = record_path(checkpoints_dir, name);
    let body = match serde_json::to_vec(record) {
        Ok(body) => body,
        Err(e) => {
            tracing::warn!(agent = %name, error = %e, "couldn't encode the deletion time");
            return;
        }
    };
    if let Err(e) = tokio::fs::write(&path, body).await {
        tracing::warn!(agent = %name, error = %e, path = %path.display(), "couldn't record when the agent was deleted; its deletion time will read as its last checkpoint");
    }
}

/// The record of the agent `name`'s deletion, when one exists.
pub(super) async fn read_deletion(checkpoints_dir: &Path, name: &str) -> Option<DeletionRecord> {
    let path = record_path(checkpoints_dir, name);
    let bytes = match tokio::fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            tracing::warn!(agent = %name, error = %e, path = %path.display(), "couldn't read when the agent was deleted");
            return None;
        }
    };
    match serde_json::from_slice::<DeletionRecord>(&bytes) {
        Ok(record) => Some(record),
        Err(e) => {
            tracing::warn!(agent = %name, error = %e, path = %path.display(), "the agent's deletion record is unreadable");
            None
        }
    }
}

/// Remove the deletion record of an agent that exists again. A failure is
/// logged: the listing skips agents that exist, so a stale record is
/// harmless until the agent is deleted again and overwrites it.
pub(super) async fn clear_deletion(checkpoints_dir: &Path, name: &str) {
    let path = record_path(checkpoints_dir, name);
    match tokio::fs::remove_file(&path).await {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            tracing::warn!(agent = %name, error = %e, path = %path.display(), "couldn't remove the agent's deletion record");
        }
    }
}
