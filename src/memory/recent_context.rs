//! Persistence for recent observation context (narrative + episode ID).
//!
//! Saved to `memory/recent_context.json` after each observation so the
//! agent can maintain conversational continuity across restarts.

use std::path::Path;

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

use crate::error::ResiduumError;

/// Persisted narrative context from the most recent observation.
///
/// Saved to `memory/recent_context.json` after each observation so the
/// agent can maintain conversational continuity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentContext {
    /// Narrative summary of the conversation at observation time.
    pub narrative: String,
    /// When this context was created.
    #[serde(with = "crate::time::minute_format")]
    pub created_at: NaiveDateTime,
    /// Episode ID that produced this context.
    pub episode_id: String,
}

/// Save a recent context to disk atomically.
///
/// # Errors
/// Returns an error if the file cannot be written.
pub async fn save_recent_context(path: &Path, ctx: &RecentContext) -> Result<(), ResiduumError> {
    let json = serde_json::to_string_pretty(ctx)
        .map_err(|e| ResiduumError::Memory(format!("failed to serialize recent context: {e}")))?;

    let dir = path.parent().ok_or_else(|| {
        ResiduumError::Memory(format!(
            "recent context path has no parent directory: {}",
            path.display()
        ))
    })?;

    let tmp_path = dir.join(".recent_context.json.tmp");

    tokio::fs::write(&tmp_path, &json).await.map_err(|e| {
        ResiduumError::Memory(format!(
            "failed to write temporary recent context at {}: {e}",
            tmp_path.display()
        ))
    })?;

    tokio::fs::rename(&tmp_path, path).await.map_err(|e| {
        ResiduumError::Memory(format!(
            "failed to rename recent context from {} to {}: {e}",
            tmp_path.display(),
            path.display()
        ))
    })?;

    Ok(())
}

/// Load a recent context from disk.
///
/// Returns `None` if the file does not exist or is empty.
///
/// # Errors
/// Returns an error if the file exists but cannot be parsed.
pub async fn load_recent_context(path: &Path) -> Result<Option<RecentContext>, ResiduumError> {
    match tokio::fs::read_to_string(path).await {
        Ok(contents) if contents.trim().is_empty() => Ok(None),
        Ok(contents) => {
            let ctx: RecentContext = serde_json::from_str(&contents).map_err(|e| {
                ResiduumError::Memory(format!(
                    "failed to parse recent context at {}: {e}",
                    path.display()
                ))
            })?;
            Ok(Some(ctx))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(ResiduumError::Memory(format!(
            "failed to read recent context at {}: {e}",
            path.display()
        ))),
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;

    #[tokio::test]
    async fn recent_context_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recent_context.json");

        let ctx = RecentContext {
            narrative: "We were discussing caching strategies.".to_string(),
            created_at: chrono::Utc::now().naive_utc(),
            episode_id: "ep-001".to_string(),
        };

        save_recent_context(&path, &ctx).await.unwrap();
        let loaded = load_recent_context(&path).await.unwrap();
        assert!(loaded.is_some(), "should load saved context");
        let loaded = loaded.unwrap();
        assert_eq!(loaded.narrative, ctx.narrative, "narrative should match");
        assert_eq!(loaded.episode_id, ctx.episode_id, "episode_id should match");
    }

    #[tokio::test]
    async fn recent_context_missing_file_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.json");
        let loaded = load_recent_context(&path).await.unwrap();
        assert!(loaded.is_none(), "missing file should return None");
    }

    #[tokio::test]
    async fn recent_context_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recent_context.json");

        let ctx1 = RecentContext {
            narrative: "first narrative".to_string(),
            created_at: chrono::Utc::now().naive_utc(),
            episode_id: "ep-001".to_string(),
        };
        save_recent_context(&path, &ctx1).await.unwrap();

        let ctx2 = RecentContext {
            narrative: "second narrative".to_string(),
            created_at: chrono::Utc::now().naive_utc(),
            episode_id: "ep-002".to_string(),
        };
        save_recent_context(&path, &ctx2).await.unwrap();

        let loaded = load_recent_context(&path).await.unwrap().unwrap();
        assert_eq!(
            loaded.narrative, "second narrative",
            "should have overwritten"
        );
    }
}
