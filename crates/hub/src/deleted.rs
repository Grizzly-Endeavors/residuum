use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use residuum_checkpoints::agent_repos_dir;

const FILE_NAME: &str = "deleted.json";

#[derive(Serialize, Deserialize)]
pub(super) struct DeletionRecord {
    pub deleted_at: DateTime<Utc>,
    #[serde(default)]
    pub checkpoint_id: Option<String>,
    #[serde(default)]
    pub role_page: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
}

fn record_path(checkpoints_dir: &Path, name: &str) -> PathBuf {
    agent_repos_dir(checkpoints_dir, name).join(FILE_NAME)
}

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
