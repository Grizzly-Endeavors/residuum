//! Session store: durable on-disk record of every run.
//!
//! Each run gets a metadata JSON file under the sessions directory,
//! organized by date (`YYYY-MM/DD/<run_id>.json`). While the run is live,
//! its transcript is durably appended to a sibling `<run_id>.transcript.jsonl`
//! file after every model response and tool result (see
//! [`TranscriptSink`]/[`RunTranscriptSink`]), so a crash mid-turn loses at
//! most the message currently in flight. On completion, the full transcript
//! is folded into the metadata file itself, so a finished run's web-facing
//! read path is a single file; the sibling JSONL is read only for runs that
//! haven't reached a terminal state — a live run's transcript in the web UI,
//! and recovery of a run whose process exited or whose task panicked.
//!
//! [`SessionStore::list_completed_runs`] pages through finished runs newest
//! first for the web UI's sessions listing.

use std::path::{Path, PathBuf};

use anyhow::Context as _;
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
    /// `None` when the run produced no episode — it staged nothing and
    /// either ended with `HEARTBEAT_OK` or its transcript fell below the
    /// skip token floor, or the merge itself failed — its transcript is
    /// still kept either way.
    #[serde(default)]
    pub episode_id: Option<String>,
    /// The run's full message transcript, filled in at completion (empty
    /// until then — the durable record while the run is live is the sibling
    /// `<run_id>.transcript.jsonl` file, appended to by
    /// [`SessionStore::append_transcript`]).
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

/// Which completed runs [`SessionStore::list_completed_runs`] includes.
#[derive(Debug, Clone, Copy, Default)]
pub struct RunFilter<'a> {
    /// Only runs in this category (a
    /// [`SessionCategory`](super::registry::SessionCategory) label).
    pub category: Option<&'a str>,
    /// Only runs at this session address.
    pub address: Option<&'a str>,
}

/// A run record as the listing reads it: every [`RunRecord`] field except
/// the transcript, which serde skips over without allocating it — a
/// completed run's record can hold a long transcript, and a listing page
/// reads dozens of records.
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
            transcript: Vec::new(),
        }
    }
}

/// Position in the newest-first listing of completed runs: everything
/// strictly older than this run (by start time, then run id) comes after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunCursor {
    /// Start time of the last run on the previous page.
    pub started_at: DateTime<Utc>,
    /// Run id of the last run on the previous page.
    pub run_id: String,
}

impl RunCursor {
    /// Encode as an opaque, URL-safe string: `<start nanos>_<run id>`.
    #[must_use]
    pub fn encode(&self) -> String {
        let nanos = self
            .started_at
            .timestamp_nanos_opt()
            .unwrap_or_else(|| self.started_at.timestamp_millis().saturating_mul(1_000_000));
        format!("{nanos}_{}", self.run_id)
    }

    /// Decode a string produced by [`Self::encode`]. `None` if it is
    /// malformed or its run id is not a valid run id.
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

/// One page of completed runs, newest first.
#[derive(Debug)]
pub struct CompletedRunPage {
    /// Completed runs' records, without their transcripts.
    pub runs: Vec<RunRecord>,
    /// Where the next page starts, or `None` if this is the last page.
    pub next: Option<RunCursor>,
}

/// Longest run id accepted from outside (the web API).
const MAX_RUN_ID_LEN: usize = 128;

/// Whether `run_id` has the shape of a run id this store can hold: ASCII
/// letters, digits, `-`, and `_` only, so it can never name a path outside
/// the store when it comes from a client.
#[must_use]
pub fn is_valid_run_id(run_id: &str) -> bool {
    !run_id.is_empty()
        && run_id.len() <= MAX_RUN_ID_LEN
        && run_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Directory entries of `dir` whose names satisfy `keep`, sorted newest
/// (lexicographically greatest) first. A missing directory reads as empty.
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

/// Whether `name` looks like a month directory (`YYYY-MM`).
fn is_month_dir(name: &str) -> bool {
    name.len() == 7
        && name.as_bytes().get(4) == Some(&b'-')
        && name
            .bytes()
            .enumerate()
            .all(|(i, b)| i == 4 || b.is_ascii_digit())
}

/// Whether `name` looks like a day directory (`DD`).
fn is_day_dir(name: &str) -> bool {
    name.len() == 2 && name.bytes().all(|b| b.is_ascii_digit())
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

    /// Build a system note and append it to `run_id`'s durable transcript
    /// sidecar, returning the message so the caller can also fold it into
    /// whatever in-memory buffer it holds for the run — a live turn's
    /// `RecentMessages`, or a freshly reloaded transcript `Vec` (panic
    /// recovery) — or discard it, when the note is a best-effort addition to
    /// a *different* session's transcript the caller doesn't own a buffer
    /// for. Shared by every place a relay/delivery failure needs to leave a
    /// visible trace in a run's own record, rather than each duplicating the
    /// append.
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

    /// Read back a run's incrementally-appended transcript, for recovering a
    /// run that never reached a terminal state on its own — whether that's
    /// startup recovery after a prior process exit, or backfilling a run
    /// whose task panicked mid-turn. Returns an empty vec if the file is
    /// missing or a line fails to parse (a torn write at the very end of the
    /// file, from a crash mid-append).
    pub(crate) async fn read_incremental_transcript(
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

    /// Find a run's metadata record file by run id, without needing to know
    /// its start date up front — mirrors
    /// [`crate::memory::episode_store::find_episode_path`]'s directory walk.
    ///
    /// Returns `Ok(None)` when the sessions directory is missing or no run
    /// with this id exists, and `Err` only on I/O failures reading
    /// directories.
    ///
    /// # Errors
    /// Returns an error if a directory cannot be read.
    pub(crate) async fn find_run_path(&self, run_id: &str) -> anyhow::Result<Option<PathBuf>> {
        if !matches!(tokio::fs::try_exists(&self.sessions_dir).await, Ok(true)) {
            return Ok(None);
        }
        let target = format!("{run_id}.json");
        find_file_by_name(&self.sessions_dir, &target).await
    }

    /// Read a run's on-disk record and transcript by run id.
    ///
    /// A completed run's transcript is already in the record; a run that
    /// hasn't reached a terminal state yet has an empty `transcript` field,
    /// so this falls back to the live incremental sidecar file in that case.
    ///
    /// # Errors
    /// Returns an error if the sessions directory cannot be read, or the
    /// record file cannot be read or parsed.
    pub(crate) async fn read_run(
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

    /// List completed runs newest first (by start time, then run id), one
    /// page of at most `limit` at a time, only those matching `filter`.
    /// `before` continues from a previous page's [`CompletedRunPage::next`].
    ///
    /// Runs that haven't completed yet are skipped — they are live, and
    /// listed from the registry instead. Walks the date directories newest
    /// first and stops as soon as the page is full, so a page costs only the
    /// days it spans. A record that can't be read or parsed is logged and
    /// skipped rather than failing the whole page.
    ///
    /// # Errors
    /// Returns an error if a sessions directory cannot be read.
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

    /// Read every completed run record in one day directory that matches
    /// `filter` and sorts strictly after `before`.
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
            if before.is_some_and(|c| (header.started_at, header.run_id.as_str()) >= c.sort_key()) {
                continue;
            }
            runs.push(RunRecord::from(header));
        }
        Ok(runs)
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

    /// Reconcile a single run record left incomplete by a prior process
    /// exit, then rewrite it as completed. Returns `true` if the file was
    /// rewritten.
    ///
    /// The metadata file's own `transcript` field is only ever populated at
    /// completion, so for a run that never got there it reads the
    /// incrementally-appended sibling file instead. The record also has no
    /// separately-stored "final turn summary" — the last message's content
    /// stands in for it, which is exactly where a session's
    /// `HEARTBEAT_OK`/`HEARTBEAT_URGENT` sentinel would appear had the
    /// process not exited before recording one explicitly.
    ///
    /// A run's completion pipeline may have already merged successfully
    /// before the process exited — a crash between that merge and this same
    /// record being written as `completed` is exactly what leaves it here.
    /// So this first asks the merge writer whether the run id is already
    /// recorded as merged and, if so, backfills the record's state and
    /// episode id from that instead of running the completion pipeline (and
    /// its LLM extraction) again, which would otherwise merge the same
    /// transcript a second time.
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

        let episode_id = match env.merge_writer.find_merged_episode(&record.run_id).await {
            Ok(Some(existing)) => {
                tracing::info!(run_id = %record.run_id, episode_id = %existing, "run already merged before the process exited; backfilling record instead of re-merging");
                Some(existing)
            }
            Ok(None) => {
                self.run_completion_pipeline(&record, &transcript, env)
                    .await
            }
            Err(e) => {
                tracing::warn!(run_id = %record.run_id, error = %e, "failed to check merged-run record, proceeding with the completion pipeline");
                self.run_completion_pipeline(&record, &transcript, env)
                    .await
            }
        };

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

    /// Run the full completion pipeline (skip check, extraction, merge) for
    /// a recovered run, using its last transcript message as the stand-in
    /// "final turn summary" (see [`Self::recover_if_incomplete`]).
    ///
    /// Only an `Assistant` message stands in as the summary. A process exit
    /// can leave the transcript ending on the run's own kickoff `User`
    /// message (a crash before the model ever replied), and that prompt
    /// routinely contains the literal string `HEARTBEAT_OK` itself — every
    /// pulse and action prompt template tells the model to return it when
    /// there's nothing to report. Treating that prompt text as the summary
    /// would make the skip check's `ends_with_sentinel` check see a false
    /// positive and silently drop a run that never got to do any work at
    /// all.
    async fn run_completion_pipeline(
        &self,
        record: &RunRecord,
        transcript: &[Message],
        env: &super::session_memory::SessionMemoryEnv<'_>,
    ) -> Option<String> {
        let summary = transcript
            .last()
            .filter(|m| m.role == crate::inference::Role::Assistant)
            .map(|m| m.content.clone())
            .unwrap_or_default();
        let tag = crate::memory::types::SourceTag::session(
            record.address.clone(),
            record.run_id.clone(),
            record.category.clone(),
        );
        super::session_memory::complete_session_memory(
            tag,
            &summary,
            transcript,
            super::session_memory::SessionMemory::new(),
            env,
        )
        .await
    }
}

/// Recursively search `dir` for a file named exactly `target`, iteratively
/// (a stack of pending directories rather than async recursion).
///
/// # Errors
/// Returns an error if a directory cannot be read.
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
            model_tier: crate::config::BackgroundModelTier::Medium,
            conversation_target: None,
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
    async fn recover_incomplete_runs_ignores_heartbeat_ok_in_the_kickoff_prompt_itself() {
        // A crash before the model ever replies leaves the transcript ending
        // on its own `User` kickoff message — and every pulse/action prompt
        // template tells the model to return HEARTBEAT_OK when there's
        // nothing to report, so that literal text routinely appears in the
        // prompt itself. The stand-in summary must not be read from a
        // non-`Assistant` message, or this pulse prompt would be
        // misdetected as the run having "ended with HEARTBEAT_OK" and a
        // substantial, never-actually-run transcript would be skipped.
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
            "a substantial transcript interrupted before any assistant reply must not be \
             skipped just because its own kickoff prompt mentions HEARTBEAT_OK"
        );
    }

    #[tokio::test]
    async fn recover_after_merge_succeeds_but_run_record_write_fails_does_not_duplicate() {
        // Simulates the exact crash window the double-merge bug lived in: the
        // live run's completion pipeline merged successfully (an episode and
        // its observations landed on disk), but the process exited before
        // `complete_run` could persist that episode id into the run's own
        // record — so on disk the record is still the empty "running" one
        // `begin_run` wrote. Recovery must detect the run was already merged
        // and backfill instead of running the pipeline again.
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

        // The live run's completion pipeline runs and merges successfully...
        let transcript = vec![
            Message::user("investigate the issue"),
            Message::assistant(big_content, None),
        ];
        let tag = crate::memory::types::SourceTag::session(
            info.address.to_string(),
            info.run_id.clone(),
            info.category.as_str(),
        );
        let episode_id = crate::background::session_memory::complete_session_memory(
            tag,
            "done",
            &transcript,
            crate::background::session_memory::SessionMemory::new(),
            &env,
        )
        .await;
        assert!(
            episode_id.is_some(),
            "the simulated live-run merge should have produced an episode"
        );

        // ...but the process exits before `complete_run` can persist that
        // episode id — the on-disk record is still the "running" one
        // `begin_run` wrote, which is exactly what makes this run look
        // incomplete to startup recovery below.

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

        let log = crate::memory::log_store::load_observation_log(&layout.observations_json())
            .await
            .unwrap();
        assert_eq!(
            log.observations.len(),
            1,
            "the observation must not be duplicated by recovery"
        );

        let latest = crate::memory::episode_store::latest_episode_id(&layout.episodes_dir())
            .await
            .unwrap();
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
                transcript.clone(),
                Some("ep-001".to_string()),
            )
            .await;

        let (record, read_transcript) = store.read_run(&info.run_id).await.unwrap().unwrap();
        assert_eq!(record.state, "completed");
        assert_eq!(record.episode_id.as_deref(), Some("ep-001"));
        assert_eq!(read_transcript.len(), transcript.len());
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
