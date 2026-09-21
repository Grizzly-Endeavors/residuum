//! Session store: durable on-disk record of every run.
//!
//! Each run gets a metadata JSON file under the sessions directory,
//! organized by date (`YYYY-MM/DD/<run_id>.json`). While the run is live,
//! its transcript is durably appended to a sibling `<run_id>.transcript.jsonl`
//! file after every model response and tool result (see
//! [`TranscriptSink`]/[`RunTranscriptSink`]), so a crash mid-turn loses at
//! most the message currently in flight. On completion, the full transcript
//! is folded into the metadata file itself, so a finished run's web-facing
//! read path is a single file; only startup recovery of a run that never
//! reached a terminal state reads the sibling JSONL.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use crate::agent::turn::TranscriptSink;
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
    /// `None` when the run produced no episode (a `HEARTBEAT_OK` ending, a
    /// transcript below the skip token floor with nothing staged, or a
    /// merge failure) — its transcript is still kept either way.
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

    /// Path a run's metadata record lives at, given its start time.
    fn run_path(&self, run_id: &str, started_at: DateTime<Utc>) -> PathBuf {
        self.sessions_dir
            .join(started_at.format("%Y-%m").to_string())
            .join(started_at.format("%d").to_string())
            .join(format!("{run_id}.json"))
    }

    /// Path a run's incrementally-appended transcript lives at.
    fn transcript_path(&self, run_id: &str, started_at: DateTime<Utc>) -> PathBuf {
        self.sessions_dir
            .join(started_at.format("%Y-%m").to_string())
            .join(started_at.format("%d").to_string())
            .join(format!("{run_id}.transcript.jsonl"))
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

    /// Append newly-produced messages to a run's incremental transcript.
    ///
    /// Called after every model response and tool result while the run is
    /// live, so a crash mid-turn loses at most the message currently in
    /// flight. Best-effort: a failure here degrades crash recovery, not the
    /// run itself, so it is logged and swallowed rather than propagated.
    pub(crate) async fn append_transcript(
        &self,
        run_id: &str,
        started_at: DateTime<Utc>,
        messages: &[Message],
    ) {
        if messages.is_empty() {
            return;
        }
        let path = self.transcript_path(run_id, started_at);
        let Some(dir) = path.parent() else {
            tracing::warn!(run_id, "transcript path has no parent directory");
            return;
        };
        if let Err(e) = tokio::fs::create_dir_all(dir).await {
            tracing::warn!(run_id, path = %path.display(), error = %e, "failed to create session transcript directory");
            return;
        }

        let mut buf = String::new();
        for message in messages {
            match serde_json::to_string(message) {
                Ok(line) => {
                    buf.push_str(&line);
                    buf.push('\n');
                }
                Err(e) => {
                    tracing::warn!(run_id, error = %e, "failed to serialize message for transcript append");
                }
            }
        }
        if buf.is_empty() {
            return;
        }

        match tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
        {
            Ok(mut file) => {
                if let Err(e) = file.write_all(buf.as_bytes()).await {
                    tracing::warn!(run_id, path = %path.display(), error = %e, "failed to append to session transcript");
                }
            }
            Err(e) => {
                tracing::warn!(run_id, path = %path.display(), error = %e, "failed to open session transcript for append");
            }
        }
    }

    /// Read back a run's incrementally-appended transcript, for startup
    /// recovery of a run that never reached a terminal state. Returns an
    /// empty vec if the file is missing or a line fails to parse (a torn
    /// write at the very end of the file, from a crash mid-append).
    async fn read_incremental_transcript(
        &self,
        run_id: &str,
        started_at: DateTime<Utc>,
    ) -> Vec<Message> {
        let path = self.transcript_path(run_id, started_at);
        match tokio::fs::read_to_string(&path).await {
            Ok(contents) => contents
                .lines()
                .filter_map(|line| match serde_json::from_str(line) {
                    Ok(msg) => Some(msg),
                    Err(e) => {
                        tracing::warn!(run_id, error = %e, "skipping unparseable transcript line during recovery");
                        None
                    }
                })
                .collect(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => {
                tracing::warn!(run_id, path = %path.display(), error = %e, "failed to read incremental transcript during recovery");
                Vec::new()
            }
        }
    }

    /// Finalize a run's record: set its terminal state, completion time,
    /// full transcript, and the episode it was merged into (if any).
    ///
    /// Returns the path the record was written to, or `None` if the write
    /// failed — callers must not report a transcript to exist when it
    /// doesn't; the failure itself is already logged with structured fields.
    pub async fn complete_run(
        &self,
        info: &SessionInfo,
        state: &str,
        transcript: Vec<Message>,
        episode_id: Option<String>,
    ) -> Option<PathBuf> {
        let path = self.run_path(&info.run_id, info.started_at);
        let mut record = RunRecord::starting(info);
        record.state = state.to_string();
        record.completed_at = Some(Utc::now());
        record.transcript = transcript;
        record.episode_id = episode_id;
        match write_record(&path, &record).await {
            Ok(()) => Some(path),
            Err(e) => {
                tracing::warn!(run_id = %info.run_id, path = %path.display(), error = %e, "failed to write completed session run record");
                None
            }
        }
    }

    /// At startup, run the full completion pipeline — skip check, final
    /// observation, and merge — for every run left incomplete by a prior
    /// process exit, from its persisted transcript, then mark it completed.
    /// Returns the number of runs recovered.
    pub(crate) async fn recover_incomplete_runs(
        &self,
        env: &super::session_memory::SessionMemoryEnv<'_>,
    ) -> usize {
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
            if self.recover_if_incomplete(&path, env).await {
                recovered += 1;
            }
        }

        recovered
    }

    /// Run the completion memory pipeline for a single run record left
    /// incomplete by a prior process exit, then rewrite it as completed.
    /// Returns `true` if the file was rewritten.
    ///
    /// The metadata file's own `transcript` field is only ever populated at
    /// completion, so for a run that never got there it reads the
    /// incrementally-appended sibling file instead. The record also has no
    /// separately-stored "final turn summary" — the last message's content
    /// stands in for it, which is exactly where a session's
    /// `HEARTBEAT_OK`/`HEARTBEAT_URGENT` sentinel would appear had the
    /// process not exited before recording one explicitly.
    async fn recover_if_incomplete(
        &self,
        path: &Path,
        env: &super::session_memory::SessionMemoryEnv<'_>,
    ) -> bool {
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
        tracing::warn!(run_id = %record.run_id, previous_state = %record.state, "recovering run left incomplete by a prior process exit");

        let transcript = self
            .read_incremental_transcript(&record.run_id, record.started_at)
            .await;
        let summary = transcript
            .last()
            .map(|m| m.content.clone())
            .unwrap_or_default();
        let tag = crate::memory::types::SourceTag::session(
            record.address.clone(),
            record.run_id.clone(),
            record.category.clone(),
        );
        let episode_id = super::session_memory::complete_session_memory(
            tag,
            &summary,
            &transcript,
            super::session_memory::SessionMemory::new(),
            env,
        )
        .await;

        record.state = "completed".to_string();
        record.completed_at = Some(Utc::now());
        record.interrupted = true;
        record.episode_id = episode_id;
        record.transcript = transcript;
        if let Err(e) = write_record(path, &record).await {
            tracing::warn!(path = %path.display(), error = %e, "failed to rewrite recovered session run record");
            return false;
        }
        true
    }
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

/// Binds a [`SessionStore`] to one run's identity, so the turn executor can
/// append to its transcript file after every model response and tool
/// result without knowing anything about sessions.
pub(crate) struct RunTranscriptSink<'a> {
    pub(crate) store: &'a SessionStore,
    pub(crate) run_id: &'a str,
    pub(crate) started_at: DateTime<Utc>,
}

#[async_trait]
impl TranscriptSink for RunTranscriptSink<'_> {
    async fn append(&self, messages: &[Message]) {
        self.store
            .append_transcript(self.run_id, self.started_at, messages)
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::background::registry::{SessionCategory, SessionState};
    use crate::background::session_memory::SessionMemoryEnv;
    use crate::bus::{EventTrigger, SessionAddress};

    /// Build a `SessionMemoryEnv` whose observer/reflector never fire, so
    /// startup recovery tests exercise the skip path deterministically.
    fn test_env() -> (
        crate::workspace::layout::WorkspaceLayout,
        std::sync::Arc<crate::memory::observer::Observer>,
        std::sync::Arc<crate::memory::merge_writer::MemoryMergeWriter>,
    ) {
        crate::background::subagent::test_memory_extras()
    }

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
        let path = store
            .complete_run(&info, "completed", transcript, None)
            .await
            .expect("write should succeed");

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
            .complete_run(&info, "completed", transcript.clone(), None)
            .await;

        let path = store.run_path(&info.run_id, info.started_at);
        let contents = tokio::fs::read_to_string(&path).await.unwrap();
        let record: RunRecord = serde_json::from_str(&contents).unwrap();
        assert_eq!(record.transcript.len(), transcript.len());
    }

    #[tokio::test]
    async fn recover_incomplete_runs_recovers_stale_running_runs() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().to_path_buf());
        let info = sample_info();
        store.begin_run(&info).await;

        let (layout, observer, merge_writer) = test_env();
        let env = SessionMemoryEnv {
            observer: &observer,
            merge_writer: &merge_writer,
            layout: &layout,
            episode_skip_token_floor: 2000,
            tz: chrono_tz::UTC,
        };
        let recovered = store.recover_incomplete_runs(&env).await;
        assert_eq!(recovered, 1);

        let path = store.run_path(&info.run_id, info.started_at);
        let contents = tokio::fs::read_to_string(&path).await.unwrap();
        let record: RunRecord = serde_json::from_str(&contents).unwrap();
        assert_eq!(record.state, "completed");
        assert!(record.interrupted);
        assert!(
            record.episode_id.is_none(),
            "an empty transcript should skip the episode, not error"
        );
    }

    #[tokio::test]
    async fn recover_incomplete_runs_leaves_completed_runs_alone() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().to_path_buf());
        let info = sample_info();
        store.begin_run(&info).await;
        store.complete_run(&info, "completed", vec![], None).await;

        let (layout, observer, merge_writer) = test_env();
        let env = SessionMemoryEnv {
            observer: &observer,
            merge_writer: &merge_writer,
            layout: &layout,
            episode_skip_token_floor: 2000,
            tz: chrono_tz::UTC,
        };
        let recovered = store.recover_incomplete_runs(&env).await;
        assert_eq!(recovered, 0);

        let path = store.run_path(&info.run_id, info.started_at);
        let contents = tokio::fs::read_to_string(&path).await.unwrap();
        let record: RunRecord = serde_json::from_str(&contents).unwrap();
        assert!(!record.interrupted);
    }

    #[tokio::test]
    async fn recover_incomplete_runs_on_missing_dir_returns_zero() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().join("does-not-exist"));
        let (layout, observer, merge_writer) = test_env();
        let env = SessionMemoryEnv {
            observer: &observer,
            merge_writer: &merge_writer,
            layout: &layout,
            episode_skip_token_floor: 2000,
            tz: chrono_tz::UTC,
        };
        assert_eq!(store.recover_incomplete_runs(&env).await, 0);
    }

    #[tokio::test]
    async fn recover_incomplete_runs_merges_a_substantial_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().to_path_buf());
        let mut info = sample_info();
        info.run_id = "run-test-substantial".to_string();
        store.begin_run(&info).await;

        // Append a transcript large enough to clear the skip floor, the way
        // a live run's turn would via `append_transcript`, simulating a
        // crash after the turn produced content but before the run reached
        // a terminal state (its metadata file is still the empty one
        // `begin_run` wrote).
        let big_content = "a".repeat(9000);
        store
            .append_transcript(
                &info.run_id,
                info.started_at,
                &[
                    Message::user("investigate the issue"),
                    Message::assistant(big_content, None),
                ],
            )
            .await;
        let path = store.run_path(&info.run_id, info.started_at);

        let layout = crate::workspace::layout::WorkspaceLayout::new(dir.path());
        let search_index = std::sync::Arc::new(
            crate::memory::search::MemoryIndex::open_or_create(&layout.search_index_dir()).unwrap(),
        );
        let reflector = crate::memory::reflector::Reflector::disabled(chrono_tz::UTC);
        let merge_writer = crate::memory::merge_writer::MemoryMergeWriter::new(
            reflector,
            layout.clone(),
            search_index,
            None,
            None,
        );
        let observer = crate::memory::observer::Observer::new(
            Box::new(crate::memory::test_helpers::MockMemoryProvider::new(
                r#"{"observations": [{"content": "recovered a finding", "timestamp": "2026-02-21T14:30", "visibility": "background"}]}"#,
            )),
            crate::memory::observer::ObserverConfig::default(),
        );
        let env = SessionMemoryEnv {
            observer: &observer,
            merge_writer: &merge_writer,
            layout: &layout,
            episode_skip_token_floor: 2000,
            tz: chrono_tz::UTC,
        };
        let recovered = store.recover_incomplete_runs(&env).await;
        assert_eq!(recovered, 1);

        let contents = tokio::fs::read_to_string(&path).await.unwrap();
        let recovered_record: RunRecord = serde_json::from_str(&contents).unwrap();
        assert!(
            recovered_record.episode_id.is_some(),
            "a substantial recovered transcript should be merged into an episode"
        );
    }

    #[tokio::test]
    async fn complete_run_returns_none_when_write_fails() {
        // A regular file sitting where the sessions directory should be
        // makes `create_dir_all` fail for every run path under it.
        let dir = tempfile::tempdir().unwrap();
        let blocked_path = dir.path().join("blocked");
        tokio::fs::write(&blocked_path, b"not a directory")
            .await
            .unwrap();
        let store = SessionStore::new(blocked_path);
        let info = sample_info();

        let result = store.complete_run(&info, "completed", vec![], None).await;
        assert!(
            result.is_none(),
            "a failed write must not report a transcript path"
        );
    }
}
