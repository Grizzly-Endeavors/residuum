//! Durable record of which session run ids have already been merged into an
//! episode.
//!
//! A session run's completion pipeline (live completion or startup recovery
//! of a run left incomplete by a prior process exit) can run more than once
//! for the same run id — a crash between a successful merge and the run
//! record being marked completed leaves the record `running`, and recovery
//! would otherwise re-run the pipeline from the same transcript. This log is
//! the durable, O(1)-lookup source of truth [`crate::memory::merge_writer::MemoryMergeWriter`]
//! consults to refuse a duplicate merge, and [`crate::background::store::SessionStore`]
//! consults to backfill a recovered run's record instead of re-merging.

use std::collections::HashMap;
use std::path::Path;

use anyhow::Context;

/// Load the run-id -> episode-id map from disk.
///
/// Returns an empty map if the file does not exist.
///
/// # Errors
/// Returns an error if the file exists but cannot be read or parsed.
pub(crate) async fn load_merged_runs(path: &Path) -> anyhow::Result<HashMap<String, String>> {
    match tokio::fs::read_to_string(path).await {
        Ok(contents) => serde_json::from_str(&contents)
            .with_context(|| format!("corrupt merged-runs record at {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(HashMap::new()),
        Err(e) => Err(anyhow::Error::new(e).context(format!(
            "failed to read merged-runs record at {}",
            path.display()
        ))),
    }
}

/// Record that `run_id` merged into `episode_id`, atomically.
///
/// Loads the existing map, inserts the new entry, and saves atomically (temp
/// file + rename) so a crash mid-write never leaves a corrupt record.
///
/// # Errors
/// Returns an error if the file cannot be read or written.
pub(crate) async fn record_merged_run(
    path: &Path,
    run_id: &str,
    episode_id: &str,
) -> anyhow::Result<()> {
    let mut map = load_merged_runs(path).await?;
    map.insert(run_id.to_string(), episode_id.to_string());
    let json =
        serde_json::to_string_pretty(&map).context("failed to serialize merged-runs record")?;
    crate::util::fs::atomic_write(path, &json).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn load_missing_file_returns_empty_map() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("merged_runs.json");

        let map = load_merged_runs(&path).await.unwrap();
        assert!(map.is_empty());
    }

    #[tokio::test]
    async fn record_and_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("merged_runs.json");

        record_merged_run(&path, "run-1", "ep-001").await.unwrap();
        record_merged_run(&path, "run-2", "ep-002").await.unwrap();

        let map = load_merged_runs(&path).await.unwrap();
        assert_eq!(map.get("run-1").map(String::as_str), Some("ep-001"));
        assert_eq!(map.get("run-2").map(String::as_str), Some("ep-002"));
    }

    #[tokio::test]
    async fn record_preserves_existing_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("merged_runs.json");

        record_merged_run(&path, "run-1", "ep-001").await.unwrap();
        record_merged_run(&path, "run-2", "ep-002").await.unwrap();

        let map = load_merged_runs(&path).await.unwrap();
        assert_eq!(
            map.len(),
            2,
            "recording a new run must not drop earlier ones"
        );
    }

    #[tokio::test]
    async fn load_corrupt_file_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("merged_runs.json");
        tokio::fs::write(&path, "not valid json").await.unwrap();

        let result = load_merged_runs(&path).await;
        assert!(result.is_err());
    }
}
