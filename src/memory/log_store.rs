//! Persistence for the observation log (`observations.json`).
//!
//! Uses atomic write (temp file + rename) to prevent corruption.

use std::path::Path;

use crate::memory::types::{Observation, ObservationLog};

/// Load the observation log from disk.
///
/// Returns an empty log if the file does not exist.
///
/// # Errors
/// Returns an error if the file exists but cannot be read or parsed.
pub async fn load_observation_log(
    path: &Path,
) -> Result<ObservationLog, crate::error::IronclawError> {
    match tokio::fs::read_to_string(path).await {
        Ok(contents) => serde_json::from_str(&contents).map_err(|e| {
            crate::error::IronclawError::Memory(format!(
                "failed to parse observation log at {}: {e}",
                path.display()
            ))
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ObservationLog::new()),
        Err(e) => Err(crate::error::IronclawError::Memory(format!(
            "failed to read observation log at {}: {e}",
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
pub async fn save_observation_log(
    path: &Path,
    log: &ObservationLog,
) -> Result<(), crate::error::IronclawError> {
    let json = serde_json::to_string_pretty(log).map_err(|e| {
        crate::error::IronclawError::Memory(format!("failed to serialize observation log: {e}"))
    })?;

    let dir = path.parent().ok_or_else(|| {
        crate::error::IronclawError::Memory(format!(
            "observation log path has no parent directory: {}",
            path.display()
        ))
    })?;

    let tmp_path = dir.join(".observations.json.tmp");

    tokio::fs::write(&tmp_path, &json).await.map_err(|e| {
        crate::error::IronclawError::Memory(format!(
            "failed to write temporary observation log at {}: {e}",
            tmp_path.display()
        ))
    })?;

    tokio::fs::rename(&tmp_path, path).await.map_err(|e| {
        crate::error::IronclawError::Memory(format!(
            "failed to rename observation log from {} to {}: {e}",
            tmp_path.display(),
            path.display()
        ))
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
) -> Result<(), crate::error::IronclawError> {
    let mut log = load_observation_log(path).await?;
    for obs in observations {
        log.push(obs);
    }
    save_observation_log(path, &log).await
}

/// Generate the next episode ID based on existing observations.
///
/// Scans `source_episodes` across all observations to find the maximum
/// `ep-NNN` ID, then returns the next one. Returns `"ep-001"` if none exist.
#[must_use]
pub fn next_episode_id(log: &ObservationLog) -> String {
    let max_num = log
        .observations
        .iter()
        .flat_map(|obs| obs.source_episodes.iter())
        .filter_map(|id| id.strip_prefix("ep-").and_then(|n| n.parse::<u32>().ok()))
        .max()
        .unwrap_or(0);

    format!("ep-{:03}", max_num + 1)
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use crate::memory::types::Visibility;
    use chrono::Utc;

    fn sample_observation(episode_id: &str) -> Observation {
        Observation {
            timestamp: Utc::now(),
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

    #[test]
    fn next_episode_id_empty() {
        let log = ObservationLog::new();
        assert_eq!(next_episode_id(&log), "ep-001", "first ID should be ep-001");
    }

    #[test]
    fn next_episode_id_increments() {
        let mut log = ObservationLog::new();
        log.push(sample_observation("ep-001"));
        log.push(sample_observation("ep-002"));
        log.push(sample_observation("ep-003"));

        assert_eq!(next_episode_id(&log), "ep-004", "should increment past max");
    }

    #[test]
    fn next_episode_id_no_source_episodes() {
        // Observations without source_episodes (e.g., from the reflector)
        // should not contribute to the episode ID counter.
        let mut log = ObservationLog::new();
        log.push(Observation {
            timestamp: Utc::now(),
            project_context: "test".to_string(),
            source_episodes: vec![],
            visibility: Visibility::User,
            content: "reflected observation".to_string(),
        });
        log.push(sample_observation("ep-005"));

        assert_eq!(
            next_episode_id(&log),
            "ep-006",
            "should find max from source_episodes"
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
