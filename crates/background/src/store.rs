use std::path::{Path, PathBuf};

use anyhow::Context as _;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use residuum_agent::turn::TranscriptSink;
use residuum_bus::{AgentResultStatus, PulseOverlap};
use residuum_inference::Message;

use residuum_bus::session::SessionInfo;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    pub address: String,
    pub run_id: String,
    pub category: String,
    pub source_label: String,
    pub spawner: Option<String>,
    pub depth: u32,
    pub purpose: String,
    pub agent_skill: Option<String>,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub state: String,
    #[serde(default)]
    pub interrupted: bool,
    #[serde(default)]
    pub episode_id: Option<String>,
    #[serde(default)]
    pub memory_pending: bool,
    #[serde(default)]
    pub transcript: Vec<Message>,
    #[serde(default)]
    pub usage: residuum_bus::usage::SessionUsageTotals,
    #[serde(default)]
    pub outcome: Option<String>,
    #[serde(default)]
    pub outcome_error: Option<String>,
    #[serde(default)]
    pub outcome_error_details: Option<String>,
    #[serde(default)]
    pub overlap: Option<PulseOverlap>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub resolved_model: Option<String>,
}

impl RunRecord {
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
            memory_pending: false,
            transcript: Vec::new(),
            usage: info.usage,
            outcome: None,
            outcome_error: None,
            outcome_error_details: None,
            overlap: info.overlap.clone(),
            model: Some(info.model.to_string()),
            resolved_model: info.resolved_model.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RunFilter<'a> {
    pub category: Option<&'a str>,
    pub address: Option<&'a str>,
    pub source_label: Option<&'a str>,
}

#[derive(Deserialize)]
struct RunRecordHeader {
    address: String,
    run_id: String,
    category: String,
    source_label: String,
    spawner: Option<String>,
    depth: u32,
    purpose: String,
    agent_skill: Option<String>,
    started_at: DateTime<Utc>,
    completed_at: Option<DateTime<Utc>>,
    state: String,
    #[serde(default)]
    interrupted: bool,
    #[serde(default)]
    episode_id: Option<String>,
    #[serde(default)]
    memory_pending: bool,
    #[serde(default)]
    usage: residuum_bus::usage::SessionUsageTotals,
    #[serde(default)]
    outcome: Option<String>,
    #[serde(default)]
    outcome_error: Option<String>,
    #[serde(default)]
    outcome_error_details: Option<String>,
    #[serde(default)]
    overlap: Option<PulseOverlap>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    resolved_model: Option<String>,
}

impl From<RunRecordHeader> for RunRecord {
    fn from(header: RunRecordHeader) -> Self {
        Self {
            address: header.address,
            run_id: header.run_id,
            category: header.category,
            source_label: header.source_label,
            spawner: header.spawner,
            depth: header.depth,
            purpose: header.purpose,
            agent_skill: header.agent_skill,
            started_at: header.started_at,
            completed_at: header.completed_at,
            state: header.state,
            interrupted: header.interrupted,
            episode_id: header.episode_id,
            memory_pending: header.memory_pending,
            transcript: Vec::new(),
            usage: header.usage,
            outcome: header.outcome,
            outcome_error: header.outcome_error,
            outcome_error_details: header.outcome_error_details,
            overlap: header.overlap,
            model: header.model,
            resolved_model: header.resolved_model,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunCursor {
    pub started_at: DateTime<Utc>,
    pub run_id: String,
}

impl RunCursor {
    #[must_use]
    pub fn encode(&self) -> String {
        let nanos = self
            .started_at
            .timestamp_nanos_opt()
            .unwrap_or_else(|| self.started_at.timestamp_millis().saturating_mul(1_000_000));
        format!("{nanos}_{}", self.run_id)
    }

    #[must_use]
    pub fn decode(raw: &str) -> Option<Self> {
        let (nanos, run_id) = raw.split_once('_')?;
        let nanos: i64 = nanos.parse().ok()?;
        if !is_valid_run_id(run_id) {
            return None;
        }
        Some(Self {
            started_at: DateTime::from_timestamp_nanos(nanos),
            run_id: run_id.to_string(),
        })
    }

    fn sort_key(&self) -> (DateTime<Utc>, &str) {
        (self.started_at, &self.run_id)
    }
}

#[derive(Debug)]
pub struct CompletedRunPage {
    pub runs: Vec<RunRecord>,
    pub next: Option<RunCursor>,
}

/// Longest run id accepted from outside (the web API).
const MAX_RUN_ID_LEN: usize = 128;

#[must_use]
pub fn is_valid_run_id(run_id: &str) -> bool {
    !run_id.is_empty()
        && run_id.len() <= MAX_RUN_ID_LEN
        && run_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

async fn sorted_dir_names_desc(
    dir: &Path,
    keep: impl Fn(&str) -> bool,
) -> anyhow::Result<Vec<String>> {
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(anyhow::Error::new(e).context(format!(
                "failed to read sessions directory {}",
                dir.display()
            )));
        }
    };
    let mut names = Vec::new();
    while let Some(entry) = entries
        .next_entry()
        .await
        .with_context(|| format!("failed to read directory entry in {}", dir.display()))?
    {
        if let Some(name) = entry.file_name().to_str()
            && keep(name)
        {
            names.push(name.to_string());
        }
    }
    names.sort_unstable_by(|a, b| b.cmp(a));
    Ok(names)
}

fn is_month_dir(name: &str) -> bool {
    name.len() == 7
        && name.as_bytes().get(4) == Some(&b'-')
        && name
            .bytes()
            .enumerate()
            .all(|(i, b)| i == 4 || b.is_ascii_digit())
}

fn is_day_dir(name: &str) -> bool {
    name.len() == 2 && name.bytes().all(|b| b.is_ascii_digit())
}

pub struct SessionStore {
    sessions_dir: PathBuf,
    #[cfg(any(test, feature = "test-support"))]
    tempdir_guard: Option<tempfile::TempDir>,
}

impl SessionStore {
    #[must_use]
    pub fn new(sessions_dir: PathBuf) -> Self {
        Self {
            sessions_dir,
            #[cfg(any(test, feature = "test-support"))]
            tempdir_guard: None,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn with_tempdir_guard(mut self, dir: tempfile::TempDir) -> Self {
        self.tempdir_guard = Some(dir);
        self
    }

    fn run_path(&self, run_id: &str, started_at: DateTime<Utc>) -> PathBuf {
        self.sessions_dir
            .join(started_at.format("%Y-%m").to_string())
            .join(started_at.format("%d").to_string())
            .join(format!("{run_id}.json"))
    }

    fn transcript_path(&self, run_id: &str, started_at: DateTime<Utc>) -> PathBuf {
        self.sessions_dir
            .join(started_at.format("%Y-%m").to_string())
            .join(started_at.format("%d").to_string())
            .join(format!("{run_id}.transcript.jsonl"))
    }

    pub async fn begin_run(&self, info: &SessionInfo) {
        let record = RunRecord::starting(info);
        let path = self.run_path(&info.run_id, info.started_at);
        if let Err(e) = write_record(&path, &record).await {
            tracing::warn!(run_id = %info.run_id, path = %path.display(), error = %e, "failed to write session run record");
        }
    }

    pub async fn append_transcript(
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

        let recorded_at = Utc::now();
        let mut buf = String::new();
        for message in messages {
            let line = TranscriptLineRef {
                message,
                recorded_at,
            };
            match serde_json::to_string(&line) {
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
                // `tokio::fs::File` completes `write_all` once the bytes are
                // queued for a blocking thread, and dropping the handle does
                // not wait for that write; `flush` does. Without it a reader
                // that follows the append can see an empty transcript.
                let written = match file.write_all(buf.as_bytes()).await {
                    Ok(()) => file.flush().await,
                    Err(e) => Err(e),
                };
                if let Err(e) = written {
                    tracing::warn!(run_id, path = %path.display(), error = %e, "failed to append to session transcript");
                }
            }
            Err(e) => {
                tracing::warn!(run_id, path = %path.display(), error = %e, "failed to open session transcript for append");
            }
        }
    }

    pub(crate) async fn append_note(
        &self,
        run_id: &str,
        started_at: DateTime<Utc>,
        note: &str,
    ) -> Message {
        let message = Message::system(note.to_string());
        self.append_transcript(run_id, started_at, std::slice::from_ref(&message))
            .await;
        message
    }

    pub async fn read_incremental_transcript(
        &self,
        run_id: &str,
        started_at: DateTime<Utc>,
    ) -> Vec<Message> {
        self.read_transcript_lines(run_id, started_at)
            .await
            .into_iter()
            .map(|line| line.message)
            .collect()
    }

    pub(crate) async fn read_transcript_lines(
        &self,
        run_id: &str,
        started_at: DateTime<Utc>,
    ) -> Vec<TranscriptLine> {
        let path = self.transcript_path(run_id, started_at);
        match tokio::fs::read_to_string(&path).await {
            Ok(contents) => contents
                .lines()
                .filter_map(|line| match serde_json::from_str(line) {
                    Ok(line) => Some(line),
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

    pub(crate) async fn find_run_path(&self, run_id: &str) -> anyhow::Result<Option<PathBuf>> {
        if !matches!(tokio::fs::try_exists(&self.sessions_dir).await, Ok(true)) {
            return Ok(None);
        }
        let target = format!("{run_id}.json");
        find_file_by_name(&self.sessions_dir, &target).await
    }

    pub async fn read_run(
        &self,
        run_id: &str,
    ) -> anyhow::Result<Option<(RunRecord, Vec<Message>)>> {
        let Some(path) = self.find_run_path(run_id).await? else {
            return Ok(None);
        };
        let contents = tokio::fs::read_to_string(&path)
            .await
            .with_context(|| format!("failed to read session run record at {}", path.display()))?;
        let record: RunRecord = serde_json::from_str(&contents)
            .with_context(|| format!("failed to parse session run record at {}", path.display()))?;
        let transcript = if record.transcript.is_empty() && record.state != "completed" {
            self.read_incremental_transcript(&record.run_id, record.started_at)
                .await
        } else {
            record.transcript.clone()
        };
        Ok(Some((record, transcript)))
    }

    pub async fn list_completed_runs(
        &self,
        filter: RunFilter<'_>,
        before: Option<&RunCursor>,
        limit: usize,
    ) -> anyhow::Result<CompletedRunPage> {
        let cursor_month = before.map(|c| c.started_at.format("%Y-%m").to_string());
        let cursor_day = before.map(|c| c.started_at.format("%d").to_string());
        let mut runs: Vec<RunRecord> = Vec::new();

        'months: for month in sorted_dir_names_desc(&self.sessions_dir, is_month_dir).await? {
            let in_cursor_month = cursor_month.as_deref() == Some(month.as_str());
            if cursor_month
                .as_deref()
                .is_some_and(|cm| month.as_str() > cm)
            {
                continue;
            }
            let month_dir = self.sessions_dir.join(&month);
            for day in sorted_dir_names_desc(&month_dir, is_day_dir).await? {
                if in_cursor_month && cursor_day.as_deref().is_some_and(|cd| day.as_str() > cd) {
                    continue;
                }
                let mut day_runs = self
                    .read_completed_day(&month_dir.join(&day), filter, before)
                    .await?;
                day_runs.sort_unstable_by(|a, b| {
                    (b.started_at, &b.run_id).cmp(&(a.started_at, &a.run_id))
                });
                runs.extend(day_runs);
                if runs.len() > limit {
                    break 'months;
                }
            }
        }

        let next = if runs.len() > limit {
            runs.truncate(limit);
            runs.last().map(|last| RunCursor {
                started_at: last.started_at,
                run_id: last.run_id.clone(),
            })
        } else {
            None
        };
        Ok(CompletedRunPage { runs, next })
    }

    async fn read_completed_day(
        &self,
        day_dir: &Path,
        filter: RunFilter<'_>,
        before: Option<&RunCursor>,
    ) -> anyhow::Result<Vec<RunRecord>> {
        let file_names = sorted_dir_names_desc(day_dir, |name| {
            Path::new(name).extension().is_some_and(|ext| ext == "json")
        })
        .await?;
        let mut runs = Vec::new();
        for name in file_names {
            let path = day_dir.join(&name);
            let contents = match tokio::fs::read_to_string(&path).await {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "failed to read session run record for listing, skipping it");
                    continue;
                }
            };
            let header: RunRecordHeader = match serde_json::from_str(&contents) {
                Ok(h) => h,
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "failed to parse session run record for listing, skipping it");
                    continue;
                }
            };
            if header.state != "completed" {
                continue;
            }
            if filter.category.is_some_and(|c| header.category != c) {
                continue;
            }
            if filter.address.is_some_and(|a| header.address != a) {
                continue;
            }
            if filter
                .source_label
                .is_some_and(|l| header.source_label != l)
            {
                continue;
            }
            if before.is_some_and(|c| (header.started_at, header.run_id.as_str()) >= c.sort_key()) {
                continue;
            }
            runs.push(RunRecord::from(header));
        }
        Ok(runs)
    }

    pub async fn complete_run(
        &self,
        info: &SessionInfo,
        state: &str,
        status: &AgentResultStatus,
        transcript: Vec<Message>,
        memory_pending: bool,
    ) -> Option<PathBuf> {
        let path = self.run_path(&info.run_id, info.started_at);
        let mut record = RunRecord::starting(info);
        record.state = state.to_string();
        record.completed_at = Some(Utc::now());
        record.transcript = transcript;
        record.memory_pending = memory_pending;
        let (outcome, outcome_error, outcome_error_details) = match status {
            AgentResultStatus::Completed => ("completed", None, None),
            AgentResultStatus::Cancelled => ("cancelled", None, None),
            AgentResultStatus::Failed { error, details } => {
                ("failed", Some(error.clone()), details.clone())
            }
        };
        record.outcome = Some(outcome.to_string());
        record.outcome_error = outcome_error;
        record.outcome_error_details = outcome_error_details;
        match write_record(&path, &record).await {
            Ok(()) => Some(path),
            Err(e) => {
                tracing::warn!(run_id = %info.run_id, path = %path.display(), error = %e, "failed to write completed session run record");
                None
            }
        }
    }

    pub async fn finish_memory_merge(&self, info: &SessionInfo, episode_id: Option<String>) {
        let path = self.run_path(&info.run_id, info.started_at);
        let mut record = match read_record(&path).await {
            Ok(record) => record,
            Err(e) => {
                tracing::warn!(run_id = %info.run_id, path = %path.display(), error = %e, "failed to read session run record to record its memory merge");
                return;
            }
        };
        record.episode_id = episode_id;
        record.memory_pending = false;
        if let Err(e) = write_record(&path, &record).await {
            tracing::warn!(run_id = %info.run_id, path = %path.display(), error = %e, "failed to write session run record with its memory merge");
        }
    }

    pub async fn recover_incomplete_runs(
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
        if record.state == "completed" && !record.memory_pending {
            return false;
        }
        let interrupted = record.state != "completed";
        if interrupted {
            tracing::warn!(run_id = %record.run_id, previous_state = %record.state, "recovering run left incomplete by a prior process exit");
        } else {
            tracing::info!(run_id = %record.run_id, "recovering memory merge left pending by a prior process exit");
        }

        let lines = self
            .read_transcript_lines(&record.run_id, record.started_at)
            .await;

        let episode_id = match env.merge_writer.find_merged_episode(&record.run_id).await {
            Ok(Some(existing)) => {
                tracing::info!(run_id = %record.run_id, episode_id = %existing, "run already merged before the process exited; backfilling record instead of re-merging");
                Some(existing)
            }
            Ok(None) => self.run_completion_pipeline(&record, &lines, env).await,
            Err(e) => {
                tracing::warn!(run_id = %record.run_id, error = %e, "failed to check merged-run record, proceeding with the completion pipeline");
                self.run_completion_pipeline(&record, &lines, env).await
            }
        };

        record.episode_id = episode_id;
        record.memory_pending = false;
        if interrupted {
            record.state = "completed".to_string();
            record.completed_at = Some(Utc::now());
            record.interrupted = true;
            record.transcript = lines.into_iter().map(|line| line.message).collect();
        }
        if let Err(e) = write_record(path, &record).await {
            tracing::warn!(path = %path.display(), error = %e, "failed to rewrite recovered session run record");
            return false;
        }
        true
    }

    async fn run_completion_pipeline(
        &self,
        record: &RunRecord,
        lines: &[TranscriptLine],
        env: &super::session_memory::SessionMemoryEnv<'_>,
    ) -> Option<String> {
        let summary = lines
            .last()
            .map(|line| &line.message)
            .filter(|m| m.role == residuum_inference::Role::Assistant)
            .map(|m| m.content.clone())
            .unwrap_or_default();
        let transcript: Vec<_> = lines.iter().map(TranscriptLine::timed).collect();
        let tag = residuum_memory::types::SourceTag::session(
            record.address.clone(),
            record.run_id.clone(),
            record.category.clone(),
        );
        super::session_memory::complete_session_memory(
            tag,
            &summary,
            &transcript,
            super::session_memory::SessionMemory::new(),
            env,
        )
        .await
    }
}

async fn find_file_by_name(dir: &Path, target: &str) -> anyhow::Result<Option<PathBuf>> {
    let mut pending = vec![dir.to_path_buf()];
    while let Some(current) = pending.pop() {
        let mut entries = tokio::fs::read_dir(&current)
            .await
            .with_context(|| format!("failed to read directory {}", current.display()))?;
        while let Some(entry) = entries
            .next_entry()
            .await
            .with_context(|| format!("failed to read directory entry in {}", current.display()))?
        {
            let is_dir = entry.file_type().await.is_ok_and(|t| t.is_dir());
            let path = entry.path();
            if is_dir {
                pending.push(path);
            } else if path.file_name().is_some_and(|n| n == target) {
                return Ok(Some(path));
            }
        }
    }
    Ok(None)
}

async fn write_record(path: &Path, record: &RunRecord) -> anyhow::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("session run path has no parent directory"))?;
    tokio::fs::create_dir_all(dir).await?;
    let json = serde_json::to_string_pretty(record)?;
    residuum_core::util::fs::atomic_write(path, &json).await?;
    Ok(())
}

async fn read_record(path: &Path) -> anyhow::Result<RunRecord> {
    let contents = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("failed to read session run record at {}", path.display()))?;
    serde_json::from_str(&contents)
        .with_context(|| format!("failed to parse session run record at {}", path.display()))
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct TranscriptLine {
    #[serde(flatten)]
    pub(crate) message: Message,
    #[serde(default)]
    pub(crate) recorded_at: Option<DateTime<Utc>>,
}

impl TranscriptLine {
    pub(crate) fn timed(&self) -> super::session_memory::TimedMessage<'_> {
        super::session_memory::TimedMessage {
            message: &self.message,
            recorded_at: self.recorded_at,
        }
    }
}

#[derive(Serialize)]
struct TranscriptLineRef<'a> {
    #[serde(flatten)]
    message: &'a Message,
    recorded_at: DateTime<Utc>,
}

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
    use crate::session_memory::SessionMemoryEnv;
    use residuum_bus::session::{SessionCategory, SessionState};
    use residuum_bus::{EventTrigger, SessionAddress};

    fn test_env() -> (
        residuum_config::WorkspaceLayout,
        std::sync::Arc<residuum_memory::observer::Observer>,
        std::sync::Arc<residuum_memory::merge_writer::MemoryMergeWriter>,
    ) {
        crate::testing::test_memory_extras()
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
            model: residuum_config::ModelSelector::MEDIUM,
            resolved_model: None,
            conversation_target: None,
            started_at: Utc::now(),
            usage: residuum_bus::usage::SessionUsageTotals::default(),
            overlap: None,
        }
    }

    #[tokio::test]
    async fn transcript_lines_record_their_time_and_old_lines_load_without_one() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().to_path_buf());
        let info = sample_info();
        let path = store.transcript_path(&info.run_id, info.started_at);
        tokio::fs::create_dir_all(path.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&path, "{\"role\":\"user\",\"content\":\"old line\"}\n")
            .await
            .unwrap();

        let before = Utc::now();
        store
            .append_transcript(
                &info.run_id,
                info.started_at,
                &[Message::assistant("new line", None)],
            )
            .await;
        let after = Utc::now();

        let lines = store
            .read_transcript_lines(&info.run_id, info.started_at)
            .await;
        let [old, new] = lines.as_slice() else {
            panic!("both lines should load, got {}", lines.len());
        };
        assert_eq!(old.message.content, "old line");
        assert_eq!(old.recorded_at, None, "an old line has no recorded time");
        assert_eq!(new.message.content, "new line");
        assert!(
            new.recorded_at
                .is_some_and(|at| at >= before && at <= after),
            "an appended line records when it was appended"
        );

        let messages = store
            .read_incremental_transcript(&info.run_id, info.started_at)
            .await;
        let contents: Vec<&str> = messages.iter().map(|m| m.content.as_str()).collect();
        assert_eq!(contents, ["old line", "new line"]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn appended_transcript_is_readable_as_soon_as_append_returns() {
        // `tokio::fs::File` hands writes to a blocking thread and can report
        // `write_all` complete before the bytes reach the file, so an append
        // that skips `flush` is not yet visible to a reader that follows it.
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().to_path_buf());
        let info = sample_info();
        for round in 0..300 {
            let run_id = format!("run-durable-{round}");
            store
                .append_transcript(&run_id, info.started_at, &[Message::user("hello")])
                .await;
            let read = store
                .read_incremental_transcript(&run_id, info.started_at)
                .await;
            assert_eq!(read.len(), 1, "append {round} was not readable on return");
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
            .complete_run(
                &info,
                "completed",
                &AgentResultStatus::Completed,
                transcript,
                false,
            )
            .await
            .expect("write should succeed");

        let contents = tokio::fs::read_to_string(&path).await.unwrap();
        let record: RunRecord = serde_json::from_str(&contents).unwrap();
        assert_eq!(record.state, "completed");
        assert!(record.completed_at.is_some());
        assert_eq!(record.transcript.len(), 2);
    }

    #[tokio::test]
    async fn complete_run_carries_the_infos_final_usage_totals() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().to_path_buf());
        let mut info = sample_info();
        store.begin_run(&info).await;

        info.usage.accumulate(Some(residuum_inference::Usage {
            uncached_input_tokens: Some(200),
            output_tokens: Some(40),
            ..Default::default()
        }));

        let path = store
            .complete_run(
                &info,
                "completed",
                &AgentResultStatus::Completed,
                Vec::new(),
                false,
            )
            .await
            .expect("write should succeed");

        let contents = tokio::fs::read_to_string(&path).await.unwrap();
        let record: RunRecord = serde_json::from_str(&contents).unwrap();
        assert_eq!(record.usage.input_tokens, 200);
        assert_eq!(record.usage.output_tokens, 40);
    }

    #[test]
    fn run_record_header_without_a_usage_field_deserializes_to_default() {
        let json = r#"{
            "address": "spawned-x-0001",
            "run_id": "run-1",
            "category": "spawned",
            "source_label": "agent:researcher",
            "spawner": null,
            "depth": 1,
            "purpose": "research",
            "agent_skill": null,
            "started_at": "2026-03-13T12:00:00Z",
            "completed_at": null,
            "state": "completed"
        }"#;
        let header: RunRecordHeader = serde_json::from_str(json).unwrap();
        let record = RunRecord::from(header);
        assert_eq!(
            record.usage,
            residuum_bus::usage::SessionUsageTotals::default()
        );
    }

    #[tokio::test]
    async fn stopped_run_keeps_its_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().to_path_buf());
        let info = sample_info();
        store.begin_run(&info).await;

        let transcript = vec![Message::user("hello")];
        store
            .complete_run(
                &info,
                "completed",
                &AgentResultStatus::Completed,
                transcript.clone(),
                false,
            )
            .await;

        let path = store.run_path(&info.run_id, info.started_at);
        let contents = tokio::fs::read_to_string(&path).await.unwrap();
        let record: RunRecord = serde_json::from_str(&contents).unwrap();
        assert_eq!(record.transcript.len(), transcript.len());
    }

    #[tokio::test]
    async fn complete_run_records_the_failure_reason() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().to_path_buf());
        let info = sample_info();
        store.begin_run(&info).await;

        let path = store
            .complete_run(
                &info,
                "completed",
                &AgentResultStatus::Failed {
                    error: "the model call timed out".to_string(),
                    details: None,
                },
                vec![],
                false,
            )
            .await
            .expect("write should succeed");

        let contents = tokio::fs::read_to_string(&path).await.unwrap();
        let record: RunRecord = serde_json::from_str(&contents).unwrap();
        assert_eq!(record.outcome.as_deref(), Some("failed"));
        assert_eq!(
            record.outcome_error.as_deref(),
            Some("the model call timed out")
        );
        assert_eq!(
            record.outcome_error_details, None,
            "no details to show should persist as None, not a guessed value"
        );
    }

    #[tokio::test]
    async fn complete_run_persists_the_full_cause_chain_alongside_the_message() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().to_path_buf());
        let info = sample_info();
        store.begin_run(&info).await;

        let path = store
            .complete_run(
                &info,
                "completed",
                &AgentResultStatus::Failed {
                    error: "the model call timed out".to_string(),
                    details: Some("connect timeout after 30s: api.example.com:443".to_string()),
                },
                vec![],
                false,
            )
            .await
            .expect("write should succeed");

        let contents = tokio::fs::read_to_string(&path).await.unwrap();
        let record: RunRecord = serde_json::from_str(&contents).unwrap();
        assert_eq!(
            record.outcome_error_details.as_deref(),
            Some("connect timeout after 30s: api.example.com:443"),
            "the full cause chain must survive alongside the plain-language message"
        );
    }

    #[test]
    fn a_record_written_before_error_details_existed_still_loads() {
        let json = serde_json::json!({
            "address": "main",
            "run_id": "20260101-000000-abcdef",
            "category": "spawned",
            "source_label": "test",
            "spawner": null,
            "depth": 0,
            "purpose": "test",
            "agent_skill": null,
            "started_at": "2026-01-01T00:00:00Z",
            "completed_at": "2026-01-01T00:01:00Z",
            "state": "completed",
            "outcome": "failed",
            "outcome_error": "the model call timed out",
        });
        let record: RunRecord =
            serde_json::from_value(json).expect("old record should deserialize");
        assert_eq!(
            record.outcome_error.as_deref(),
            Some("the model call timed out")
        );
        assert_eq!(
            record.outcome_error_details, None,
            "a field missing from an old record must default to None, not fail to load"
        );
    }

    #[tokio::test]
    async fn complete_run_records_a_cancelled_outcome_with_no_error() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().to_path_buf());
        let info = sample_info();
        store.begin_run(&info).await;

        let path = store
            .complete_run(
                &info,
                "completed",
                &AgentResultStatus::Cancelled,
                vec![],
                false,
            )
            .await
            .expect("write should succeed");

        let contents = tokio::fs::read_to_string(&path).await.unwrap();
        let record: RunRecord = serde_json::from_str(&contents).unwrap();
        assert_eq!(record.outcome.as_deref(), Some("cancelled"));
        assert!(record.outcome_error.is_none());
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
        store
            .complete_run(
                &info,
                "completed",
                &AgentResultStatus::Completed,
                vec![],
                false,
            )
            .await;

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

        let layout = residuum_config::WorkspaceLayout::new(dir.path());
        let search_index = std::sync::Arc::new(
            residuum_memory::search::MemoryIndex::open_or_create(&layout.search_index_dir())
                .unwrap(),
        );
        let reflector = residuum_memory::reflector::Reflector::disabled(chrono_tz::UTC);
        let merge_writer = residuum_memory::merge_writer::MemoryMergeWriter::new(
            reflector,
            layout.clone(),
            search_index,
            None,
            None,
        );
        let observer = residuum_memory::observer::Observer::new(
            Box::new(residuum_memory::testing::MockMemoryProvider::new(
                r#"{"observations": [{"content": "recovered a finding", "timestamp": "2026-02-21T14:30", "visibility": "background"}]}"#,
            )),
            residuum_memory::observer::ObserverConfig::default(),
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
    async fn recover_incomplete_runs_finishes_a_merge_left_pending_by_a_completed_run() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().to_path_buf());
        let mut info = sample_info();
        info.run_id = "run-merge-pending".to_string();
        store.begin_run(&info).await;
        store
            .append_transcript(
                &info.run_id,
                info.started_at,
                &[
                    Message::user("investigate the issue"),
                    Message::assistant("a".repeat(9000), None),
                ],
            )
            .await;
        store
            .complete_run(
                &info,
                "completed",
                &AgentResultStatus::Completed,
                vec![],
                true,
            )
            .await;

        let layout = residuum_config::WorkspaceLayout::new(dir.path());
        let search_index = std::sync::Arc::new(
            residuum_memory::search::MemoryIndex::open_or_create(&layout.search_index_dir())
                .unwrap(),
        );
        let merge_writer = residuum_memory::merge_writer::MemoryMergeWriter::new(
            residuum_memory::reflector::Reflector::disabled(chrono_tz::UTC),
            layout.clone(),
            search_index,
            None,
            None,
        );
        let observer = residuum_memory::observer::Observer::new(
            Box::new(residuum_memory::testing::MockMemoryProvider::new(
                r#"{"observations": [{"content": "recovered a finding", "timestamp": "2026-02-21T14:30", "visibility": "background"}]}"#,
            )),
            residuum_memory::observer::ObserverConfig::default(),
        );
        let env = SessionMemoryEnv {
            observer: &observer,
            merge_writer: &merge_writer,
            layout: &layout,
            episode_skip_token_floor: 2000,
            tz: chrono_tz::UTC,
        };
        assert_eq!(store.recover_incomplete_runs(&env).await, 1);

        let (record, _) = store.read_run(&info.run_id).await.unwrap().unwrap();
        assert!(
            !record.memory_pending,
            "the merge must no longer be pending"
        );
        assert!(
            record.episode_id.is_some(),
            "the pending merge must be finished and its episode recorded"
        );
        assert!(
            !record.interrupted,
            "a run that completed on its own is not interrupted by the merge's recovery"
        );
        assert_eq!(record.outcome.as_deref(), Some("completed"));
    }

    #[tokio::test]
    async fn recover_incomplete_runs_ignores_heartbeat_ok_in_the_kickoff_prompt_itself() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().to_path_buf());
        let mut info = sample_info();
        info.run_id = "run-test-kickoff-only".to_string();
        store.begin_run(&info).await;

        let padding = "a".repeat(9000);
        let kickoff = format!(
            "You are running a scheduled pulse check. If nothing actionable was \
             found, return the exact string HEARTBEAT_OK. {padding}"
        );
        store
            .append_transcript(&info.run_id, info.started_at, &[Message::user(kickoff)])
            .await;
        let path = store.run_path(&info.run_id, info.started_at);

        let layout = residuum_config::WorkspaceLayout::new(dir.path());
        let search_index = std::sync::Arc::new(
            residuum_memory::search::MemoryIndex::open_or_create(&layout.search_index_dir())
                .unwrap(),
        );
        let reflector = residuum_memory::reflector::Reflector::disabled(chrono_tz::UTC);
        let merge_writer = residuum_memory::merge_writer::MemoryMergeWriter::new(
            reflector,
            layout.clone(),
            search_index,
            None,
            None,
        );
        let observer = residuum_memory::observer::Observer::new(
            Box::new(residuum_memory::testing::MockMemoryProvider::new(
                r#"{"observations": [{"content": "recovered a finding", "timestamp": "2026-02-21T14:30", "visibility": "background"}]}"#,
            )),
            residuum_memory::observer::ObserverConfig::default(),
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
            "a substantial transcript interrupted before any assistant reply must not be \
             skipped just because its own kickoff prompt mentions HEARTBEAT_OK"
        );
    }

    #[tokio::test]
    async fn recover_after_merge_succeeds_but_run_record_write_fails_does_not_duplicate() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().to_path_buf());
        let mut info = sample_info();
        info.run_id = "run-crash-after-merge".to_string();
        store.begin_run(&info).await;

        let big_content = "a".repeat(9000);
        store
            .append_transcript(
                &info.run_id,
                info.started_at,
                &[
                    Message::user("investigate the issue"),
                    Message::assistant(big_content.clone(), None),
                ],
            )
            .await;

        let layout = residuum_config::WorkspaceLayout::new(dir.path());
        let search_index = std::sync::Arc::new(
            residuum_memory::search::MemoryIndex::open_or_create(&layout.search_index_dir())
                .unwrap(),
        );
        let reflector = residuum_memory::reflector::Reflector::disabled(chrono_tz::UTC);
        let merge_writer = residuum_memory::merge_writer::MemoryMergeWriter::new(
            reflector,
            layout.clone(),
            search_index,
            None,
            None,
        );
        let observer = residuum_memory::observer::Observer::new(
            Box::new(residuum_memory::testing::MockMemoryProvider::new(
                r#"{"observations": [{"content": "recovered a finding", "timestamp": "2026-02-21T14:30", "visibility": "background"}]}"#,
            )),
            residuum_memory::observer::ObserverConfig::default(),
        );
        let env = SessionMemoryEnv {
            observer: &observer,
            merge_writer: &merge_writer,
            layout: &layout,
            episode_skip_token_floor: 2000,
            tz: chrono_tz::UTC,
        };

        let transcript = vec![
            Message::user("investigate the issue"),
            Message::assistant(big_content, None),
        ];
        let tag = residuum_memory::types::SourceTag::session(
            info.address.to_string(),
            info.run_id.clone(),
            info.category.as_str(),
        );
        let episode_id = crate::session_memory::complete_session_memory(
            tag,
            "done",
            &crate::session_memory::untimed(&transcript),
            crate::session_memory::SessionMemory::new(),
            &env,
        )
        .await;
        assert!(
            episode_id.is_some(),
            "the simulated live-run merge should have produced an episode"
        );

        let recovered = store.recover_incomplete_runs(&env).await;
        assert_eq!(recovered, 1);

        let path = store.run_path(&info.run_id, info.started_at);
        let contents = tokio::fs::read_to_string(&path).await.unwrap();
        let record: RunRecord = serde_json::from_str(&contents).unwrap();
        assert_eq!(record.state, "completed");
        assert_eq!(
            record.episode_id, episode_id,
            "recovery should backfill the existing episode id, not mint a new one"
        );

        let log = residuum_memory::log_store::load_observation_log(&layout.observations_json())
            .await
            .unwrap();
        assert_eq!(
            log.observations.len(),
            1,
            "the observation must not be duplicated by recovery"
        );

        let latest =
            residuum_memory::episode_store::latest_episode_id(&layout.episodes_dir()).unwrap();
        assert_eq!(
            latest, episode_id,
            "recovery must not mint a second episode for the same run"
        );
    }

    #[tokio::test]
    async fn find_run_path_locates_a_run_without_knowing_its_date() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().to_path_buf());
        let info = sample_info();
        store.begin_run(&info).await;

        let found = store.find_run_path(&info.run_id).await.unwrap();
        assert!(found.is_some(), "should find the run by id alone");
        assert!(found.unwrap().ends_with(format!("{}.json", info.run_id)));
    }

    #[tokio::test]
    async fn find_run_path_returns_none_for_unknown_run() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().to_path_buf());
        assert!(
            store
                .find_run_path("run-does-not-exist")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn find_run_path_on_missing_sessions_dir_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().join("does-not-exist"));
        assert!(store.find_run_path("run-anything").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn read_run_returns_the_completed_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().to_path_buf());
        let info = sample_info();
        store.begin_run(&info).await;
        let transcript = vec![Message::user("hello"), Message::assistant("hi", None)];
        store
            .complete_run(
                &info,
                "completed",
                &AgentResultStatus::Completed,
                transcript.clone(),
                false,
            )
            .await;

        let (record, read_transcript) = store.read_run(&info.run_id).await.unwrap().unwrap();
        assert_eq!(record.state, "completed");
        assert_eq!(read_transcript.len(), transcript.len());
    }

    #[tokio::test]
    async fn finish_memory_merge_records_the_episode_and_clears_pending() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().to_path_buf());
        let info = sample_info();
        store.begin_run(&info).await;
        store
            .complete_run(
                &info,
                "completed",
                &AgentResultStatus::Completed,
                vec![Message::user("hello"), Message::assistant("hi", None)],
                true,
            )
            .await;

        let (pending, _) = store.read_run(&info.run_id).await.unwrap().unwrap();
        assert!(
            pending.memory_pending,
            "a run whose merge is queued is pending"
        );
        assert_eq!(pending.episode_id, None);

        store
            .finish_memory_merge(&info, Some("ep-001".to_string()))
            .await;

        let (record, transcript) = store.read_run(&info.run_id).await.unwrap().unwrap();
        assert!(!record.memory_pending);
        assert_eq!(record.episode_id.as_deref(), Some("ep-001"));
        assert_eq!(
            transcript.len(),
            2,
            "finishing the merge must keep the transcript"
        );
    }

    #[tokio::test]
    async fn read_run_falls_back_to_the_incremental_transcript_for_a_live_run() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().to_path_buf());
        let info = sample_info();
        store.begin_run(&info).await;
        store
            .append_transcript(
                &info.run_id,
                info.started_at,
                &[Message::user("still running")],
            )
            .await;

        let (record, transcript) = store.read_run(&info.run_id).await.unwrap().unwrap();
        assert_eq!(record.state, "running");
        assert_eq!(transcript.len(), 1);
        assert_eq!(transcript.first().unwrap().content, "still running");
    }

    #[tokio::test]
    async fn read_run_returns_none_for_unknown_run() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().to_path_buf());
        assert!(
            store
                .read_run("run-does-not-exist")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn complete_run_returns_none_when_write_fails() {
        let dir = tempfile::tempdir().unwrap();
        let blocked_path = dir.path().join("blocked");
        tokio::fs::write(&blocked_path, b"not a directory")
            .await
            .unwrap();
        let store = SessionStore::new(blocked_path);
        let info = sample_info();

        let result = store
            .complete_run(
                &info,
                "completed",
                &AgentResultStatus::Completed,
                vec![],
                false,
            )
            .await;
        assert!(
            result.is_none(),
            "a failed write must not report a transcript path"
        );
    }
}
