//! Persistence for the observation log (`observations.json`).
//!
//! Uses atomic write (temp file + rename) to prevent corruption.

use std::path::Path;

use anyhow::Context;

use crate::memory::types::{Observation, ObservationLog};

/// Load the observation log from disk.
///
/// Returns an empty log if the file does not exist.
///
/// # Errors
/// Returns an error if the file exists but cannot be read or parsed.
pub async fn load_observation_log(path: &Path) -> anyhow::Result<ObservationLog> {
    match tokio::fs::read_to_string(path).await {
        Ok(contents) => serde_json::from_str(&contents).with_context(|| {
            format!(
                "corrupt observation log on disk at {} \
                 (a .json.bak backup may exist alongside it with a valid prior version)",
                path.display()
            )
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ObservationLog::new()),
        Err(e) => Err(anyhow::Error::new(e).context(format!(
            "failed to read observation log at {}",
            path.display()
        ))),
    }
}

/// Save the observation log to disk atomically.
///
/// Writes to a temporary file in the same directory, then renames
/// to avoid partial writes on crash.
///
/// # Errors
/// Returns an error if the file cannot be written.
pub async fn save_observation_log(path: &Path, log: &ObservationLog) -> anyhow::Result<()> {
    let json = serde_json::to_string_pretty(log).context("failed to serialize observation log")?;

    let dir = path.parent().ok_or_else(|| {
        anyhow::anyhow!(
            "observation log path has no parent directory: {}",
            path.display()
        )
    })?;

    let tmp_path = dir.join(".observations.json.tmp");

    tokio::fs::write(&tmp_path, &json).await.with_context(|| {
        format!(
            "failed to write temporary observation log at {}",
            tmp_path.display()
        )
    })?;

    tokio::fs::rename(&tmp_path, path).await.with_context(|| {
        format!(
            "failed to rename observation log from {} to {}",
            tmp_path.display(),
            path.display()
        )
    })?;

    Ok(())
}

/// Append observations to the observation log on disk.
///
/// Loads the existing log, extends with new observations, and saves atomically.
///
/// # Errors
/// Returns an error if loading or saving fails.
pub async fn append_observations(
    path: &Path,
    observations: Vec<Observation>,
) -> anyhow::Result<()> {
    let mut log = load_observation_log(path).await?;
    for obs in observations {
        log.push(obs);
    }
    save_observation_log(path, &log).await
}

/// Generate the next episode ID by scanning the episodes directory for existing JSONL files.
///
/// Walks `episodes_dir` recursively for files named `ep-NNN.jsonl`, takes the max `NNN`,
/// and returns `ep-(max+1)` zero-padded to 3 digits. Returns `"ep-001"` if none exist.
/// JSONL transcripts persist even after reflection, making this a stable counter.
///
/// # Errors
/// Returns an error if the directory cannot be read.
pub async fn next_episode_id(episodes_dir: &Path) -> anyhow::Result<String> {
    let max_num = max_episode_num(episodes_dir)?;
    let id = format!("ep-{:03}", max_num + 1);
    tracing::debug!(episode_id = %id, "assigned next episode ID");
    Ok(id)
}

/// Save per-episode observations to an archive file atomically.
///
/// Serializes `observations` as a JSON array and writes atomically via a temp file rename.
///
/// # Errors
/// Returns an error if the file cannot be written.
pub(crate) async fn save_episode_observations(
    path: &Path,
    observations: &[Observation],
) -> anyhow::Result<()> {
    let json =
        serde_json::to_string(observations).context("failed to serialize episode observations")?;

    let dir = path.parent().ok_or_else(|| {
        anyhow::anyhow!(
            "episode obs path has no parent directory: {}",
            path.display()
        )
    })?;

    let tmp_path = dir.join(".obs.json.tmp");

    tokio::fs::write(&tmp_path, &json).await.with_context(|| {
        format!(
            "failed to write episode observations at {}",
            tmp_path.display()
        )
    })?;

    tokio::fs::rename(&tmp_path, path).await.with_context(|| {
        format!(
            "failed to rename episode observations from {} to {}",
            tmp_path.display(),
            path.display()
        )
    })?;

    Ok(())
}

/// Recursively walk `dir` for `ep-NNN.jsonl` files and return the maximum `NNN` found.
///
/// Returns `0` if the directory does not exist or contains no matching files.
fn max_episode_num(dir: &Path) -> anyhow::Result<u32> {
    if !dir.exists() {
        return Ok(0);
    }
    let mut max = 0_u32;
    walk_for_max(dir, &mut max)?;
    Ok(max)
}

fn walk_for_max(dir: &Path, max: &mut u32) -> anyhow::Result<()> {
    let entries = std::fs::read_dir(dir)
        .with_context(|| format!("failed to read episodes directory {}", dir.display()))?;

    for entry in entries {
        let entry = entry.context("failed to read directory entry")?;
        let path = entry.path();

        if path.is_dir() {
            walk_for_max(&path, max)?;
        } else if path.extension().is_some_and(|ext| ext == "jsonl")
            && let Some(n) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| s.strip_prefix("ep-"))
                .and_then(|s| s.parse::<u32>().ok())
        {
            *max = (*max).max(n);
        }
    }

    Ok(())
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use crate::memory::types::Visibility;
    fn sample_observation(episode_id: &str) -> Observation {
        Observation {
            timestamp: chrono::Utc::now().naive_utc(),
            project_context: "test".to_string(),
            source_episodes: vec![episode_id.to_string()],
            visibility: Visibility::User,
            content: format!("observed something from {episode_id}"),
        }
    }

    #[tokio::test]
    async fn round_trip_save_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("observations.json");

        let mut log = ObservationLog::new();
        log.push(sample_observation("ep-001"));

        save_observation_log(&path, &log).await.unwrap();
        let loaded = load_observation_log(&path).await.unwrap();

        assert_eq!(loaded.len(), 1, "should load one observation");
        assert_eq!(
            loaded
                .observations
                .first()
                .and_then(|o| o.source_episodes.first().map(String::as_str)),
            Some("ep-001"),
            "source episode ID should match"
        );
    }

    #[tokio::test]
    async fn load_missing_file_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.json");

        let log = load_observation_log(&path).await.unwrap();
        assert!(log.is_empty(), "missing file should return empty log");
    }

    #[tokio::test]
    async fn append_creates_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("observations.json");

        append_observations(&path, vec![sample_observation("ep-001")])
            .await
            .unwrap();

        let log = load_observation_log(&path).await.unwrap();
        assert_eq!(log.len(), 1, "should have one observation after append");
    }

    #[tokio::test]
    async fn append_preserves_existing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("observations.json");

        append_observations(&path, vec![sample_observation("ep-001")])
            .await
            .unwrap();
        append_observations(&path, vec![sample_observation("ep-002")])
            .await
            .unwrap();

        let log = load_observation_log(&path).await.unwrap();
        assert_eq!(
            log.len(),
            2,
            "should have two observations after two appends"
        );
    }

    #[tokio::test]
    async fn next_episode_id_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let episodes_dir = dir.path().join("episodes");
        tokio::fs::create_dir_all(&episodes_dir).await.unwrap();

        let id = next_episode_id(&episodes_dir).await.unwrap();
        assert_eq!(id, "ep-001", "empty dir should return ep-001");
    }

    #[tokio::test]
    async fn next_episode_id_missing_dir() {
        let dir = tempfile::tempdir().unwrap();
        let episodes_dir = dir.path().join("episodes");
        // Dir does not exist — should still return ep-001
        let id = next_episode_id(&episodes_dir).await.unwrap();
        assert_eq!(id, "ep-001", "missing dir should return ep-001");
    }

    #[tokio::test]
    async fn next_episode_id_scans_jsonl_files() {
        let dir = tempfile::tempdir().unwrap();
        let episodes_dir = dir.path().join("episodes");
        let month_dir = episodes_dir.join("2026-02/19");
        tokio::fs::create_dir_all(&month_dir).await.unwrap();

        // Write some dummy .jsonl files
        tokio::fs::write(month_dir.join("ep-001.jsonl"), "")
            .await
            .unwrap();
        tokio::fs::write(month_dir.join("ep-003.jsonl"), "")
            .await
            .unwrap();

        let id = next_episode_id(&episodes_dir).await.unwrap();
        assert_eq!(id, "ep-004", "should find max and increment");
    }

    #[tokio::test]
    async fn save_and_load_episode_observations() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ep-001.obs.json");

        let observations = vec![sample_observation("ep-001"), sample_observation("ep-001")];

        save_episode_observations(&path, &observations)
            .await
            .unwrap();

        let raw = tokio::fs::read_to_string(&path).await.unwrap();
        let loaded: Vec<Observation> = serde_json::from_str(&raw).unwrap();

        assert_eq!(loaded.len(), 2, "should round-trip two observations");
        assert_eq!(
            loaded.first().map(|o| o.content.as_str()),
            Some("observed something from ep-001"),
            "content should match"
        );
    }

    #[tokio::test]
    async fn atomic_write_no_tmp_file_remains() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("observations.json");

        let log = ObservationLog::new();
        save_observation_log(&path, &log).await.unwrap();

        let tmp_path = dir.path().join(".observations.json.tmp");
        assert!(
            !tmp_path.exists(),
            "tmp file should not remain after successful save"
        );
    }
}
