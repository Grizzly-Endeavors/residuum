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

    crate::util::fs::atomic_write(path, &json).await
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

    crate::util::fs::atomic_write(path, &json).await
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

    #[tokio::test]
    async fn load_observation_log_corrupt_json_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("observations.json");
        tokio::fs::write(&path, "not valid json").await.unwrap();
        let result = load_observation_log(&path).await;
        assert!(result.is_err(), "corrupt JSON should return Err");
    }
}
