//! Durable per-attempt usage records: every model call's attempts, attributed
//! to the component that made the call and the model that answered.
//!
//! Records are JSON lines appended to one file per UTC month in the agent's
//! workspace ([`WorkspaceLayout::usage_records_dir`], `YYYY-MM.jsonl`). The
//! files only grow; a line torn by a crash mid-append is skipped on read.

use std::io::SeekFrom;
use std::path::{Path, PathBuf};

use anyhow::Context;
use chrono::{DateTime, Datelike, Utc};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

use crate::error::InferenceError;
use crate::types::{AttemptOutcome, AttemptUsage, InferenceProvider, InferenceResponse, Usage};
use residuum_config::WorkspaceLayout;

/// Provider name recorded for an attempt whose provider did not say which
/// provider it was.
const UNKNOWN_PROVIDER: &str = "unknown";

/// Serializes every append in the process, so records from concurrent calls
/// (the main turn, sessions, memory and the subconscious all call models at
/// once) never interleave inside one line, whichever recorder wrote them.
static APPEND_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// What made a model call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageComponent {
    /// A main-agent turn.
    Main,
    /// A session run's turn; `kind` is what started the run (`pulse`,
    /// `action`, `agent`, `webhook`, `conversation`, `artifact`).
    Session { kind: String },
    /// Observation extraction, for the main agent or a finished session run.
    Observer,
    /// Observation log compression.
    Reflector,
    /// The subconscious classifier, mid-turn or at the end of a turn.
    Subconscious,
    /// The Auto Mode decision model.
    SystemOne,
    /// A one-shot model call made through the web model endpoint.
    WebModel,
}

impl UsageComponent {
    /// The component's name without its details, as records are filtered by.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Session { .. } => "session",
            Self::Observer => "observer",
            Self::Reflector => "reflector",
            Self::Subconscious => "subconscious",
            Self::SystemOne => "system_one",
            Self::WebModel => "web_model",
        }
    }
}

/// One attempt of one model call, as stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageRecord {
    /// When the call finished.
    pub recorded_at: DateTime<Utc>,
    /// What made the call.
    pub component: UsageComponent,
    /// The session run id the call belongs to, or the artifact a web model
    /// call was made for; `None` for the agent's own conversation and memory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation: Option<String>,
    /// The provider the attempt went to.
    pub provider: String,
    /// The model the attempt asked for.
    pub model: String,
    /// How the attempt ended.
    pub outcome: AttemptOutcome,
    /// Token usage the attempt reported; `None` when it reported none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

/// Writes and reads one agent workspace's usage records. Cheap to clone and
/// to build from a layout wherever a call is made.
#[derive(Debug, Clone)]
pub struct UsageRecorder {
    dir: PathBuf,
}

impl UsageRecorder {
    /// The recorder for the workspace at `layout`.
    #[must_use]
    pub fn new(layout: &WorkspaceLayout) -> Self {
        Self {
            dir: layout.usage_records_dir(),
        }
    }

    /// Record every attempt of one finished model call made through
    /// `provider`, successful or not.
    ///
    /// Never fails the call: a record that can't be written is logged at
    /// `warn` and dropped.
    pub async fn record_call(
        &self,
        component: &UsageComponent,
        conversation: Option<&str>,
        provider: &dyn InferenceProvider,
        result: &Result<InferenceResponse, InferenceError>,
    ) {
        let attempts = call_attempts(provider, result);
        self.record_attempts(component, conversation, &attempts)
            .await;
    }

    /// Record `attempts`, in order, as made now.
    pub async fn record_attempts(
        &self,
        component: &UsageComponent,
        conversation: Option<&str>,
        attempts: &[AttemptUsage],
    ) {
        self.record_attempts_at(Utc::now(), component, conversation, attempts)
            .await;
    }

    async fn record_attempts_at(
        &self,
        recorded_at: DateTime<Utc>,
        component: &UsageComponent,
        conversation: Option<&str>,
        attempts: &[AttemptUsage],
    ) {
        let mut lines = String::new();
        for attempt in attempts {
            let record = UsageRecord {
                recorded_at,
                component: component.clone(),
                conversation: conversation.map(str::to_string),
                provider: attempt.provider.clone(),
                model: attempt.model.clone(),
                outcome: attempt.outcome,
                usage: attempt.usage,
            };
            match serde_json::to_string(&record) {
                Ok(line) => {
                    lines.push_str(&line);
                    lines.push('\n');
                }
                Err(e) => tracing::warn!(
                    error = %e,
                    component = component.name(),
                    model = %attempt.model,
                    "failed to serialize a usage record"
                ),
            }
        }
        if lines.is_empty() {
            return;
        }
        let path = self.month_file(recorded_at);
        if let Err(e) = append_lines(&path, &lines).await {
            tracing::warn!(
                error = %e,
                path = %path.display(),
                component = component.name(),
                "failed to append usage records"
            );
        }
    }

    /// Load the records made in `from..to`, optionally only those of the
    /// component named `component` (see [`UsageComponent::name`]), oldest
    /// file first.
    ///
    /// A line that doesn't parse (one torn by a crash mid-append) is skipped
    /// with a `warn`; a month with no file has no records.
    ///
    /// # Errors
    /// Returns an error when a month's file exists but can't be read.
    pub async fn load(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        component: Option<&str>,
    ) -> anyhow::Result<Vec<UsageRecord>> {
        let mut records = Vec::new();
        if from >= to {
            return Ok(records);
        }
        let first = (from.year(), from.month());
        let last = (to.year(), to.month());
        let months = self.months_on_disk().await?;
        for (year, month) in months
            .into_iter()
            .filter(|month| (first..=last).contains(month))
        {
            let path = self.dir.join(month_file_name(year, month));
            let contents = match tokio::fs::read_to_string(&path).await {
                Ok(contents) => contents,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => {
                    return Err(e).with_context(|| {
                        format!("failed to read usage records at {}", path.display())
                    });
                }
            };
            records.extend(
                parse_lines(&path, &contents)
                    .filter(|r| r.recorded_at >= from && r.recorded_at < to)
                    .filter(|r| component.is_none_or(|name| r.component.name() == name)),
            );
        }
        Ok(records)
    }

    fn month_file(&self, at: DateTime<Utc>) -> PathBuf {
        self.dir.join(month_file_name(at.year(), at.month()))
    }

    /// The `(year, month)` of every month file in the records directory,
    /// oldest first; none when the directory doesn't exist yet.
    async fn months_on_disk(&self) -> anyhow::Result<Vec<(i32, u32)>> {
        let mut entries = match tokio::fs::read_dir(&self.dir).await {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => {
                return Err(e).with_context(|| {
                    format!("failed to list usage records in {}", self.dir.display())
                });
            }
        };
        let mut months = Vec::new();
        while let Some(entry) = entries
            .next_entry()
            .await
            .with_context(|| format!("failed to list usage records in {}", self.dir.display()))?
        {
            if let Some(month) = entry.file_name().to_str().and_then(parse_month_file_name) {
                months.push(month);
            }
        }
        months.sort_unstable();
        Ok(months)
    }
}

/// The attempts a finished call reports. A response from a provider that
/// sends no request of its own (a test double, the null provider) lists
/// none and counts as one attempt of the provider's model; an error listing
/// none sent no request, so there is nothing to record.
fn call_attempts(
    provider: &dyn InferenceProvider,
    result: &Result<InferenceResponse, InferenceError>,
) -> Vec<AttemptUsage> {
    match result {
        Ok(response) if response.attempts.is_empty() => vec![AttemptUsage {
            provider: UNKNOWN_PROVIDER.to_string(),
            model: provider.model_name().to_string(),
            outcome: AttemptOutcome::Succeeded,
            usage: response.usage,
        }],
        Ok(response) => response.attempts.clone(),
        Err(error) => error.attempts().to_vec(),
    }
}

fn month_file_name(year: i32, month: u32) -> String {
    format!("{year:04}-{month:02}.jsonl")
}

/// The `(year, month)` a [`month_file_name`] names; `None` for any other file.
fn parse_month_file_name(name: &str) -> Option<(i32, u32)> {
    let (year, month) = name.strip_suffix(".jsonl")?.split_once('-')?;
    let year = year.parse().ok()?;
    let month = month.parse().ok().filter(|m| (1..=12).contains(m))?;
    (month_file_name(year, month) == name).then_some((year, month))
}

fn parse_lines<'a>(path: &'a Path, contents: &'a str) -> impl Iterator<Item = UsageRecord> + 'a {
    contents
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .filter_map(move |(index, line)| match serde_json::from_str(line) {
            Ok(record) => Some(record),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    path = %path.display(),
                    line = index + 1,
                    "skipping an unreadable usage record"
                );
                None
            }
        })
}

/// Append `lines` to the file at `path`, first ending a line a crash left
/// unterminated so the new records start on a line of their own.
async fn append_lines(path: &Path, lines: &str) -> std::io::Result<()> {
    let _guard = APPEND_LOCK.lock().await;
    if let Some(dir) = path.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(path)
        .await?;
    let mut buf = String::with_capacity(lines.len() + 1);
    if file.metadata().await?.len() > 0 {
        file.seek(SeekFrom::End(-1)).await?;
        let mut last = [0u8; 1];
        file.read_exact(&mut last).await?;
        if last != [b'\n'] {
            buf.push('\n');
        }
    }
    buf.push_str(lines);
    file.write_all(buf.as_bytes()).await?;
    // `tokio::fs::File` hands the write to a blocking thread; `flush` waits
    // for it, so a reader that follows sees the records.
    file.flush().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CompletionOptions, Message, ToolDefinition};
    use async_trait::async_trait;
    use chrono::TimeZone;

    struct FixedProvider(Result<InferenceResponse, InferenceError>);

    #[async_trait]
    impl InferenceProvider for FixedProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _options: &CompletionOptions,
        ) -> Result<InferenceResponse, InferenceError> {
            match &self.0 {
                Ok(response) => Ok(response.clone()),
                Err(_) => Err(InferenceError::Api("refused".to_string())),
            }
        }

        fn model_name(&self) -> &'static str {
            "fixed-model"
        }
    }

    fn usage(input: u32, output: u32) -> Usage {
        Usage {
            uncached_input_tokens: Some(input),
            output_tokens: Some(output),
            ..Usage::default()
        }
    }

    fn attempt(model: &str, outcome: AttemptOutcome, usage: Option<Usage>) -> AttemptUsage {
        AttemptUsage {
            provider: "anthropic".to_string(),
            model: model.to_string(),
            outcome,
            usage,
        }
    }

    fn recorder() -> (tempfile::TempDir, UsageRecorder) {
        let dir = tempfile::tempdir().unwrap();
        let recorder = UsageRecorder::new(&WorkspaceLayout::new(dir.path()));
        (dir, recorder)
    }

    fn at(year: i32, month: u32, day: u32, hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(year, month, day, hour, 0, 0).unwrap()
    }

    fn all_time() -> (DateTime<Utc>, DateTime<Utc>) {
        (DateTime::<Utc>::MIN_UTC, DateTime::<Utc>::MAX_UTC)
    }

    async fn load_all(recorder: &UsageRecorder) -> Vec<UsageRecord> {
        let (from, to) = all_time();
        recorder.load(from, to, None).await.unwrap()
    }

    #[tokio::test]
    async fn every_attempt_of_a_call_is_recorded_in_order() {
        let (_dir, recorder) = recorder();
        let mut response = InferenceResponse::new("ok".to_string(), vec![]);
        response.attempts = vec![
            attempt("primary", AttemptOutcome::Failed, Some(usage(40, 0))),
            attempt("fallback", AttemptOutcome::Succeeded, Some(usage(100, 7))),
        ];
        let provider = FixedProvider(Ok(response.clone()));
        recorder
            .record_call(
                &UsageComponent::Session {
                    kind: "pulse".to_string(),
                },
                Some("run-1"),
                &provider,
                &Ok(response),
            )
            .await;

        let records = load_all(&recorder).await;
        assert_eq!(records.len(), 2, "one record per attempt");
        let failed = records.first().unwrap();
        assert_eq!(failed.model, "primary");
        assert_eq!(failed.outcome, AttemptOutcome::Failed);
        assert_eq!(
            failed.usage,
            Some(usage(40, 0)),
            "a failed attempt keeps what it spent"
        );
        let answered = records.get(1).unwrap();
        assert_eq!(answered.model, "fallback");
        assert_eq!(answered.provider, "anthropic");
        assert_eq!(answered.outcome, AttemptOutcome::Succeeded);
        assert_eq!(answered.usage, Some(usage(100, 7)));
        assert_eq!(
            answered.component,
            UsageComponent::Session {
                kind: "pulse".to_string()
            }
        );
        assert_eq!(answered.conversation.as_deref(), Some("run-1"));
    }

    #[tokio::test]
    async fn a_call_reporting_no_attempts_counts_as_one_of_the_providers_model() {
        let (_dir, recorder) = recorder();
        let mut response = InferenceResponse::new("ok".to_string(), vec![]);
        response.usage = Some(usage(12, 3));
        let provider = FixedProvider(Ok(response.clone()));
        recorder
            .record_call(&UsageComponent::Observer, None, &provider, &Ok(response))
            .await;

        let records = load_all(&recorder).await;
        assert_eq!(records.len(), 1);
        let record = records.first().unwrap();
        assert_eq!(record.component, UsageComponent::Observer);
        assert_eq!(record.model, "fixed-model");
        assert_eq!(record.provider, UNKNOWN_PROVIDER);
        assert_eq!(record.outcome, AttemptOutcome::Succeeded);
        assert_eq!(record.usage, Some(usage(12, 3)));
        assert_eq!(record.conversation, None);
    }

    #[tokio::test]
    async fn a_failed_call_records_every_attempt_it_carries() {
        let (_dir, recorder) = recorder();
        let provider = FixedProvider(Err(InferenceError::Api("refused".to_string())));
        let error = InferenceError::Attempted {
            error: Box::new(InferenceError::Timeout(30)),
            attempts: vec![
                attempt("primary", AttemptOutcome::Failed, Some(usage(500, 0))),
                attempt("fallback", AttemptOutcome::Failed, None),
            ],
        };
        recorder
            .record_call(&UsageComponent::Reflector, None, &provider, &Err(error))
            .await;

        let records = load_all(&recorder).await;
        assert_eq!(records.len(), 2, "one record per attempt the error carries");
        let first = records.first().unwrap();
        assert_eq!(first.component, UsageComponent::Reflector);
        assert_eq!(first.model, "primary");
        assert_eq!(first.outcome, AttemptOutcome::Failed);
        assert_eq!(
            first.usage,
            Some(usage(500, 0)),
            "what a failed attempt spent is kept"
        );
        assert_eq!(records.get(1).unwrap().model, "fallback");
    }

    #[tokio::test]
    async fn a_call_that_failed_before_sending_anything_records_nothing() {
        let (_dir, recorder) = recorder();
        let provider = FixedProvider(Err(InferenceError::Api("refused".to_string())));
        recorder
            .record_call(
                &UsageComponent::Reflector,
                None,
                &provider,
                &Err(InferenceError::Parse("bad request".to_string())),
            )
            .await;
        assert!(
            load_all(&recorder).await.is_empty(),
            "a bare error sent no request"
        );
    }

    #[tokio::test]
    async fn records_roll_over_to_a_new_file_each_month() {
        let (dir, recorder) = recorder();
        let one = [attempt("m", AttemptOutcome::Succeeded, Some(usage(1, 1)))];
        recorder
            .record_attempts_at(at(2026, 10, 31, 23), &UsageComponent::Main, None, &one)
            .await;
        recorder
            .record_attempts_at(at(2026, 11, 1, 0), &UsageComponent::Main, None, &one)
            .await;
        recorder
            .record_attempts_at(at(2026, 12, 31, 23), &UsageComponent::Main, None, &one)
            .await;
        recorder
            .record_attempts_at(at(2027, 1, 1, 0), &UsageComponent::Main, None, &one)
            .await;

        let usage_dir = WorkspaceLayout::new(dir.path()).usage_records_dir();
        for name in [
            "2026-10.jsonl",
            "2026-11.jsonl",
            "2026-12.jsonl",
            "2027-01.jsonl",
        ] {
            let contents = tokio::fs::read_to_string(usage_dir.join(name))
                .await
                .unwrap();
            assert_eq!(
                contents.lines().count(),
                1,
                "{name} holds its month's record"
            );
        }

        let across = recorder
            .load(at(2026, 10, 31, 0), at(2026, 11, 2, 0), None)
            .await
            .unwrap();
        assert_eq!(
            across.len(),
            2,
            "a range spanning two months reads both files"
        );
        let year_end = recorder
            .load(at(2026, 12, 1, 0), at(2027, 2, 1, 0), None)
            .await
            .unwrap();
        assert_eq!(
            year_end.len(),
            2,
            "a range spanning the year end reads both files"
        );
    }

    #[tokio::test]
    async fn load_keeps_only_the_range_and_component_asked_for() {
        let (_dir, recorder) = recorder();
        let one = [attempt("m", AttemptOutcome::Succeeded, None)];
        recorder
            .record_attempts_at(at(2026, 11, 1, 1), &UsageComponent::Main, None, &one)
            .await;
        recorder
            .record_attempts_at(at(2026, 11, 1, 2), &UsageComponent::Observer, None, &one)
            .await;
        recorder
            .record_attempts_at(at(2026, 11, 1, 3), &UsageComponent::Main, None, &one)
            .await;

        let in_range = recorder
            .load(at(2026, 11, 1, 1), at(2026, 11, 1, 3), None)
            .await
            .unwrap();
        assert_eq!(
            in_range.iter().map(|r| r.recorded_at).collect::<Vec<_>>(),
            vec![at(2026, 11, 1, 1), at(2026, 11, 1, 2)],
            "the range includes its start and excludes its end"
        );
        let (from, to) = all_time();
        let observer = recorder.load(from, to, Some("observer")).await.unwrap();
        assert_eq!(observer.len(), 1);
        assert_eq!(
            observer.first().unwrap().component,
            UsageComponent::Observer
        );
    }

    #[tokio::test]
    async fn a_torn_last_line_is_skipped_and_later_records_still_read() {
        let (dir, recorder) = recorder();
        let one = [attempt("m", AttemptOutcome::Succeeded, Some(usage(5, 5)))];
        recorder
            .record_attempts_at(at(2026, 11, 1, 1), &UsageComponent::Main, None, &one)
            .await;
        let path = WorkspaceLayout::new(dir.path())
            .usage_records_dir()
            .join("2026-11.jsonl");
        let mut contents = tokio::fs::read_to_string(&path).await.unwrap();
        contents.push_str(r#"{"recorded_at":"2026-11-01T01:30:00Z","compo"#);
        tokio::fs::write(&path, &contents).await.unwrap();

        let (from, to) = all_time();
        let before = recorder.load(from, to, None).await.unwrap();
        assert_eq!(before.len(), 1, "the torn line is skipped, not fatal");

        recorder
            .record_attempts_at(at(2026, 11, 1, 2), &UsageComponent::Main, None, &one)
            .await;
        let after = recorder.load(from, to, None).await.unwrap();
        assert_eq!(
            after.len(),
            2,
            "a record appended after a torn line starts on its own line"
        );
    }

    #[test]
    fn only_month_files_are_read() {
        assert_eq!(parse_month_file_name("2026-11.jsonl"), Some((2026, 11)));
        assert_eq!(parse_month_file_name("2026-13.jsonl"), None);
        assert_eq!(parse_month_file_name("2026-1.jsonl"), None);
        assert_eq!(parse_month_file_name("2026-11.jsonl.tmp"), None);
        assert_eq!(parse_month_file_name("notes.jsonl"), None);
    }

    #[tokio::test]
    async fn the_widest_range_reads_every_month_on_disk() {
        let (_dir, recorder) = recorder();
        let one = [attempt("m", AttemptOutcome::Succeeded, None)];
        recorder
            .record_attempts_at(at(2020, 3, 1, 0), &UsageComponent::Main, None, &one)
            .await;
        recorder
            .record_attempts_at(at(2026, 11, 1, 0), &UsageComponent::Main, None, &one)
            .await;
        let records = recorder
            .load(DateTime::<Utc>::MIN_UTC, DateTime::<Utc>::MAX_UTC, None)
            .await
            .unwrap();
        assert_eq!(records.len(), 2);
    }

    #[tokio::test]
    async fn a_month_with_no_file_has_no_records() {
        let (_dir, recorder) = recorder();
        let (from, to) = all_time();
        assert!(recorder.load(from, to, None).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn concurrent_calls_write_whole_lines() {
        let (dir, recorder) = recorder();
        let calls = (0..32).map(|i| {
            let recorder = recorder.clone();
            tokio::spawn(async move {
                let one = [attempt(
                    &format!("m{i}"),
                    AttemptOutcome::Succeeded,
                    Some(usage(i, i)),
                )];
                recorder
                    .record_attempts_at(at(2026, 11, 1, 1), &UsageComponent::Main, None, &one)
                    .await;
            })
        });
        for call in calls.collect::<Vec<_>>() {
            call.await.unwrap();
        }
        // A second recorder for the same workspace shares the append lock.
        let other = UsageRecorder::new(&WorkspaceLayout::new(dir.path()));
        let (from, to) = all_time();
        assert_eq!(other.load(from, to, None).await.unwrap().len(), 32);
    }

    #[test]
    fn a_record_without_usage_or_conversation_stays_compact() {
        let record = UsageRecord {
            recorded_at: at(2026, 11, 1, 0),
            component: UsageComponent::Session {
                kind: "agent".to_string(),
            },
            conversation: None,
            provider: "openai".to_string(),
            model: "gpt".to_string(),
            outcome: AttemptOutcome::Failed,
            usage: None,
        };
        let line = serde_json::to_string(&record).unwrap();
        assert_eq!(
            line,
            r#"{"recorded_at":"2026-11-01T00:00:00Z","component":{"session":{"kind":"agent"}},"provider":"openai","model":"gpt","outcome":"failed"}"#
        );
        assert_eq!(serde_json::from_str::<UsageRecord>(&line).unwrap(), record);
    }
}
