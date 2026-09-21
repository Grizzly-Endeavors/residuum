//! Session store: durable on-disk record of every run.
//!
//! Each run gets one JSON file under the sessions directory, organized by
//! date (`YYYY-MM/DD/<run_id>.json`), holding metadata plus the run's
//! transcript. Stopped runs keep their transcripts; only completion status
//! and the transcript content change between the run's start and its end.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::inference::Message;

use super::registry::SessionInfo;

/// On-disk record for a single run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    /// Address of the owning session.
    pub address: String,
    /// Unique run identifier.
    pub run_id: String,
    /// `"scheduled"`, `"external"`, or `"spawned"`.
    pub category: String,
    /// Human-readable source label (e.g. `"pulse:email_check"`).
    pub source_label: String,
    /// The agent that started this session, if any.
    pub spawner: Option<String>,
    /// Depth from the main agent (main = 0).
    pub depth: u32,
    /// One-line description of what this run is doing.
    pub purpose: String,
    /// Skill the run executed with, if any.
    pub agent_skill: Option<String>,
    /// When the run started.
    pub started_at: DateTime<Utc>,
    /// When the run reached a terminal state, if it has.
    pub completed_at: Option<DateTime<Utc>>,
    /// Lifecycle state at the time this record was last written.
    pub state: String,
    /// `true` when this run was marked completed at startup because the
    /// process exited before it reached a terminal state on its own.
    #[serde(default)]
    pub interrupted: bool,
    /// The episode this run's observations were merged into, once merged.
    /// Always `None` until Phase 2 wires up the memory merge.
    #[serde(default)]
    pub episode_id: Option<String>,
    /// The run's message transcript, appended as the run progresses.
    #[serde(default)]
    pub transcript: Vec<Message>,
}

impl RunRecord {
    /// Build the initial record for a run that has just started.
    #[must_use]
    pub fn starting(info: &SessionInfo) -> Self {
        Self {
            address: info.address.to_string(),
            run_id: info.run_id.clone(),
            category: info.category.as_str().to_string(),
            source_label: info.source_label.clone(),
            spawner: info.spawner.as_ref().map(ToString::to_string),
            depth: info.depth,
            purpose: info.purpose.clone(),
            agent_skill: info.agent_skill.as_ref().map(|s| s.as_ref().to_string()),
            started_at: info.started_at,
            completed_at: None,
            state: "running".to_string(),
            interrupted: false,
            episode_id: None,
            transcript: Vec::new(),
        }
    }
}

/// Durable on-disk record of every session run.
pub struct SessionStore {
    sessions_dir: PathBuf,
}

impl SessionStore {
    /// Create a store rooted at the given sessions directory.
    #[must_use]
    pub fn new(sessions_dir: PathBuf) -> Self {
        Self { sessions_dir }
    }

    /// Path a run's record lives at, given its start time.
    fn run_path(&self, run_id: &str, started_at: DateTime<Utc>) -> PathBuf {
        self.sessions_dir
            .join(started_at.format("%Y-%m").to_string())
            .join(started_at.format("%d").to_string())
            .join(format!("{run_id}.json"))
    }

    /// Write the initial record for a run that has just started.
    ///
    /// Best-effort: failures are logged and otherwise swallowed so a disk
    /// problem degrades observability, not session execution.
    pub async fn begin_run(&self, info: &SessionInfo) {
        let record = RunRecord::starting(info);
        let path = self.run_path(&info.run_id, info.started_at);
        if let Err(e) = write_record(&path, &record).await {
            tracing::warn!(run_id = %info.run_id, path = %path.display(), error = %e, "failed to write session run record");
        }
    }

    /// Finalize a run's record: set its terminal state, completion time, and
    /// full transcript.
    ///
    /// Best-effort, like [`begin_run`](Self::begin_run).
    pub async fn complete_run(
        &self,
        info: &SessionInfo,
        state: &str,
        transcript: Vec<Message>,
    ) -> PathBuf {
        let path = self.run_path(&info.run_id, info.started_at);
        let mut record = RunRecord::starting(info);
        record.state = state.to_string();
        record.completed_at = Some(Utc::now());
        record.transcript = transcript;
        if let Err(e) = write_record(&path, &record).await {
            tracing::warn!(run_id = %info.run_id, path = %path.display(), error = %e, "failed to write completed session run record");
        }
        path
    }

    /// At startup, mark every run left incomplete by a prior process exit as
    /// completed, so it stops appearing to be in progress.
    ///
    /// Memory merging for these runs arrives in Phase 2; here they are only
    /// marked, not observed. Returns the number of runs recovered.
    pub async fn mark_incomplete_as_completed(&self) -> usize {
        let mut recovered = 0;
        let mut month_dirs = match tokio::fs::read_dir(&self.sessions_dir).await {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return 0,
            Err(e) => {
                tracing::warn!(path = %self.sessions_dir.display(), error = %e, "failed to read sessions directory at startup");
                return 0;
            }
        };

        let mut run_files = Vec::new();
        while let Ok(Some(month_entry)) = month_dirs.next_entry().await {
            let Ok(mut day_dirs) = tokio::fs::read_dir(month_entry.path()).await else {
                continue;
            };
            while let Ok(Some(day_entry)) = day_dirs.next_entry().await {
                let Ok(mut files) = tokio::fs::read_dir(day_entry.path()).await else {
                    continue;
                };
                while let Ok(Some(file_entry)) = files.next_entry().await {
                    let path = file_entry.path();
                    if path.extension().is_some_and(|ext| ext == "json") {
                        run_files.push(path);
                    }
                }
            }
        }

        for path in run_files {
            if recover_if_incomplete(&path).await {
                recovered += 1;
            }
        }

        recovered
    }
}

/// Rewrite a single run file as completed if it wasn't already terminal.
/// Returns `true` if the file was rewritten.
async fn recover_if_incomplete(path: &Path) -> bool {
    let contents = match tokio::fs::read_to_string(path).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "failed to read session run record at startup");
            return false;
        }
    };
    let mut record: RunRecord = match serde_json::from_str(&contents) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "failed to parse session run record at startup");
            return false;
        }
    };
    if record.state == "completed" {
        return false;
    }
    tracing::warn!(run_id = %record.run_id, previous_state = %record.state, "marking run left incomplete by a prior process exit as completed");
    record.state = "completed".to_string();
    record.completed_at = Some(Utc::now());
    record.interrupted = true;
    if let Err(e) = write_record(path, &record).await {
        tracing::warn!(path = %path.display(), error = %e, "failed to rewrite recovered session run record");
        return false;
    }
    true
}

/// Serialize and atomically write a run record.
async fn write_record(path: &Path, record: &RunRecord) -> anyhow::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("session run path has no parent directory"))?;
    tokio::fs::create_dir_all(dir).await?;
    let json = serde_json::to_string_pretty(record)?;
    crate::util::fs::atomic_write(path, &json).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::background::registry::{SessionCategory, SessionState};
    use crate::bus::{EventTrigger, SessionAddress};

    fn sample_info() -> SessionInfo {
        SessionInfo {
            address: SessionAddress::from("spawned-researcher-0001"),
            run_id: "run-test-1".to_string(),
            category: SessionCategory::Spawned,
            trigger: EventTrigger::Agent,
            source_label: "agent:researcher".to_string(),
            state: SessionState::Running,
            spawner: Some(SessionAddress::from("main")),
            depth: 1,
            purpose: "research the thing".to_string(),
            agent_skill: None,
            started_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn begin_run_writes_running_record() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().to_path_buf());
        let info = sample_info();

        store.begin_run(&info).await;

        let path = store.run_path(&info.run_id, info.started_at);
        let contents = tokio::fs::read_to_string(&path).await.unwrap();
        let record: RunRecord = serde_json::from_str(&contents).unwrap();
        assert_eq!(record.state, "running");
        assert_eq!(record.run_id, "run-test-1");
        assert!(record.transcript.is_empty());
    }

    #[tokio::test]
    async fn complete_run_overwrites_with_transcript_and_state() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().to_path_buf());
        let info = sample_info();
        store.begin_run(&info).await;

        let transcript = vec![Message::user("hello"), Message::assistant("hi", None)];
        let path = store.complete_run(&info, "completed", transcript).await;

        let contents = tokio::fs::read_to_string(&path).await.unwrap();
        let record: RunRecord = serde_json::from_str(&contents).unwrap();
        assert_eq!(record.state, "completed");
        assert!(record.completed_at.is_some());
        assert_eq!(record.transcript.len(), 2);
    }

    #[tokio::test]
    async fn stopped_run_keeps_its_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().to_path_buf());
        let info = sample_info();
        store.begin_run(&info).await;

        let transcript = vec![Message::user("hello")];
        store
            .complete_run(&info, "completed", transcript.clone())
            .await;

        let path = store.run_path(&info.run_id, info.started_at);
        let contents = tokio::fs::read_to_string(&path).await.unwrap();
        let record: RunRecord = serde_json::from_str(&contents).unwrap();
        assert_eq!(record.transcript.len(), transcript.len());
    }

    #[tokio::test]
    async fn mark_incomplete_as_completed_recovers_stale_running_runs() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().to_path_buf());
        let info = sample_info();
        store.begin_run(&info).await;

        let recovered = store.mark_incomplete_as_completed().await;
        assert_eq!(recovered, 1);

        let path = store.run_path(&info.run_id, info.started_at);
        let contents = tokio::fs::read_to_string(&path).await.unwrap();
        let record: RunRecord = serde_json::from_str(&contents).unwrap();
        assert_eq!(record.state, "completed");
        assert!(record.interrupted);
    }

    #[tokio::test]
    async fn mark_incomplete_as_completed_leaves_completed_runs_alone() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().to_path_buf());
        let info = sample_info();
        store.begin_run(&info).await;
        store.complete_run(&info, "completed", vec![]).await;

        let recovered = store.mark_incomplete_as_completed().await;
        assert_eq!(recovered, 0);

        let path = store.run_path(&info.run_id, info.started_at);
        let contents = tokio::fs::read_to_string(&path).await.unwrap();
        let record: RunRecord = serde_json::from_str(&contents).unwrap();
        assert!(!record.interrupted);
    }

    #[tokio::test]
    async fn mark_incomplete_as_completed_on_missing_dir_returns_zero() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().join("does-not-exist"));
        assert_eq!(store.mark_incomplete_as_completed().await, 0);
    }
}
