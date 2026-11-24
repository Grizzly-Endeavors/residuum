use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use residuum_bus::usage::SessionUsageTotals;
use residuum_config::WorkspaceLayout;
use residuum_inference::{
    InferenceError, InferenceProvider, InferenceResponse, Usage, UsageComponent, UsageRecorder,
};

#[derive(Debug, Clone, Copy, Default)]
pub struct TurnUsage {
    pub output_tokens: u32,
    pub has_usage: bool,
    pub tool_calls: u32,
}

impl TurnUsage {
    pub fn accumulate(&mut self, usage: Option<Usage>) {
        let Some(usage) = usage else {
            return;
        };
        self.has_usage = true;
        self.output_tokens += usage.output_tokens.unwrap_or(0);
    }

    pub fn record_tool_calls(&mut self, n: u32) {
        self.tool_calls += n;
    }
}

#[async_trait]
pub trait UsageSink: Send + Sync {
    async fn record_call(
        &self,
        provider: &dyn InferenceProvider,
        result: &Result<InferenceResponse, InferenceError>,
    );

    async fn accumulate(&self, usage: Option<Usage>) -> SessionUsageTotals;

    async fn record_tool_calls(&self, n: u32) -> SessionUsageTotals;
}

pub async fn load_session_usage_totals(path: &Path) -> SessionUsageTotals {
    match tokio::fs::read_to_string(path).await {
        Ok(contents) if contents.trim().is_empty() => SessionUsageTotals::default(),
        Ok(contents) => serde_json::from_str(&contents).unwrap_or_else(|e| {
            tracing::warn!(
                error = %e,
                path = %path.display(),
                "failed to parse persisted usage totals; starting from zero"
            );
            SessionUsageTotals::default()
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => SessionUsageTotals::default(),
        Err(e) => {
            tracing::warn!(
                error = %e,
                path = %path.display(),
                "failed to read persisted usage totals; starting from zero"
            );
            SessionUsageTotals::default()
        }
    }
}

pub async fn save_session_usage_totals(path: &Path, totals: &SessionUsageTotals) {
    let json = match serde_json::to_string(totals) {
        Ok(j) => j,
        Err(e) => {
            tracing::warn!(error = %e, "failed to serialize usage totals");
            return;
        }
    };
    if let Err(e) = residuum_core::util::fs::atomic_write(path, json).await {
        tracing::warn!(error = format!("{e:#}"), path = %path.display(), "failed to persist usage totals");
    }
}

pub struct MainUsageSink {
    totals: Arc<Mutex<SessionUsageTotals>>,
    persist_path: Option<std::path::PathBuf>,
    recorder: Option<UsageRecorder>,
}

impl MainUsageSink {
    #[must_use]
    pub fn new(totals: Arc<Mutex<SessionUsageTotals>>, layout: Option<&WorkspaceLayout>) -> Self {
        Self {
            totals,
            persist_path: layout.map(WorkspaceLayout::usage_totals_json),
            recorder: layout.map(UsageRecorder::new),
        }
    }
}

#[async_trait]
impl UsageSink for MainUsageSink {
    async fn record_call(
        &self,
        provider: &dyn InferenceProvider,
        result: &Result<InferenceResponse, InferenceError>,
    ) {
        if let Some(recorder) = &self.recorder {
            recorder
                .record_call(&UsageComponent::Main, None, provider, result)
                .await;
        }
    }

    async fn accumulate(&self, usage: Option<Usage>) -> SessionUsageTotals {
        let snapshot = {
            let mut guard = self.totals.lock().await;
            guard.accumulate(usage);
            *guard
        };
        if let Some(path) = &self.persist_path {
            save_session_usage_totals(path, &snapshot).await;
        }
        snapshot
    }

    async fn record_tool_calls(&self, n: u32) -> SessionUsageTotals {
        let snapshot = {
            let mut guard = self.totals.lock().await;
            guard.record_tool_calls(n);
            *guard
        };
        if let Some(path) = &self.persist_path {
            save_session_usage_totals(path, &snapshot).await;
        }
        snapshot
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(input: u32, output: u32) -> Usage {
        Usage {
            uncached_input_tokens: Some(input),
            output_tokens: Some(output),
            ..Usage::default()
        }
    }

    #[test]
    fn turn_usage_accumulates_output_tokens_across_calls() {
        let mut turn = TurnUsage::default();
        turn.accumulate(Some(usage(100, 20)));
        turn.accumulate(Some(usage(150, 30)));
        assert_eq!(turn.output_tokens, 50);
        assert!(turn.has_usage);
    }

    #[test]
    fn turn_usage_none_leaves_totals_unchanged_but_not_marked_has_usage() {
        let mut turn = TurnUsage::default();
        turn.accumulate(None);
        assert_eq!(turn.output_tokens, 0);
        assert!(
            !turn.has_usage,
            "a provider reporting no usage must not flip has_usage"
        );
    }

    #[test]
    fn turn_usage_has_usage_stays_true_once_set_even_if_a_later_call_has_none() {
        let mut turn = TurnUsage::default();
        turn.accumulate(Some(usage(10, 5)));
        turn.accumulate(None);
        assert!(
            turn.has_usage,
            "one call with usage is enough to know some usage occurred"
        );
        assert_eq!(
            turn.output_tokens, 5,
            "the None call must not erase prior totals"
        );
    }

    #[test]
    fn turn_usage_record_tool_calls_accumulates_across_batches() {
        let mut turn = TurnUsage::default();
        turn.record_tool_calls(3);
        turn.record_tool_calls(2);
        assert_eq!(turn.tool_calls, 5);
    }

    #[test]
    fn turn_usage_with_zero_tool_calls_stays_zero() {
        let turn = TurnUsage::default();
        assert_eq!(turn.tool_calls, 0);
    }

    #[tokio::test]
    async fn load_session_usage_totals_missing_file_returns_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage_totals.json");
        let totals = load_session_usage_totals(&path).await;
        assert_eq!(totals, SessionUsageTotals::default());
    }

    #[tokio::test]
    async fn load_session_usage_totals_corrupt_file_returns_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage_totals.json");
        tokio::fs::write(&path, "not json").await.unwrap();
        let totals = load_session_usage_totals(&path).await;
        assert_eq!(totals, SessionUsageTotals::default());
    }

    #[tokio::test]
    async fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage_totals.json");
        let mut totals = SessionUsageTotals::default();
        totals.accumulate(Some(Usage {
            uncached_input_tokens: Some(200),
            cache_read_tokens: Some(1_000),
            cache_write_tokens: Some(50),
            output_tokens: Some(40),
            ..Usage::default()
        }));
        save_session_usage_totals(&path, &totals).await;
        let loaded = load_session_usage_totals(&path).await;
        assert_eq!(loaded, totals);
        assert_eq!(
            (loaded.cache_read_tokens, loaded.cache_write_tokens),
            (1_000, 50),
            "the cache totals survive a restart"
        );
    }

    #[tokio::test]
    async fn load_session_usage_totals_reads_a_file_written_before_the_cache_totals() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage_totals.json");
        tokio::fs::write(
            &path,
            r#"{"input_tokens":900,"output_tokens":80,"context_tokens":300,"tool_calls":4}"#,
        )
        .await
        .unwrap();
        let loaded = load_session_usage_totals(&path).await;
        assert_eq!(
            loaded,
            SessionUsageTotals {
                input_tokens: 900,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                output_tokens: 80,
                context_tokens: Some(300),
                tool_calls: 4,
            },
            "an old totals file keeps its figures instead of resetting to zero"
        );
    }

    #[tokio::test]
    async fn save_session_usage_totals_replaces_the_file_through_a_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage_totals.json");
        tokio::fs::write(
            &path,
            "{\"stale\": true, \"padding\": \"a long earlier file\"}",
        )
        .await
        .unwrap();
        let mut totals = SessionUsageTotals::default();
        totals.accumulate(Some(usage(5, 1)));
        save_session_usage_totals(&path, &totals).await;

        assert_eq!(load_session_usage_totals(&path).await, totals);
        let mut names = Vec::new();
        let mut entries = tokio::fs::read_dir(dir.path()).await.unwrap();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
        assert_eq!(
            names,
            ["usage_totals.json"],
            "the write renames its temp file into place and leaves nothing behind"
        );
    }

    #[tokio::test]
    async fn main_usage_sink_persists_after_every_call() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        let path = layout.usage_totals_json();
        tokio::fs::create_dir_all(layout.memory_dir())
            .await
            .unwrap();
        let shared = Arc::new(Mutex::new(SessionUsageTotals::default()));
        let sink = MainUsageSink::new(Arc::clone(&shared), Some(&layout));

        let first = sink.accumulate(Some(usage(100, 20))).await;
        assert_eq!(first.input_tokens, 100);
        let persisted_after_first = load_session_usage_totals(&path).await;
        assert_eq!(
            persisted_after_first, first,
            "every call should write through to disk"
        );

        let second = sink.accumulate(Some(usage(50, 10))).await;
        assert_eq!(
            second.input_tokens, 150,
            "totals accumulate across calls on the shared Arc"
        );
        let persisted_after_second = load_session_usage_totals(&path).await;
        assert_eq!(persisted_after_second, second);
    }

    #[tokio::test]
    async fn main_usage_sink_with_no_persist_path_still_accumulates_in_memory() {
        let shared = Arc::new(Mutex::new(SessionUsageTotals::default()));
        let sink = MainUsageSink::new(Arc::clone(&shared), None);
        sink.accumulate(Some(usage(10, 5))).await;
        let totals = sink.accumulate(Some(usage(10, 5))).await;
        assert_eq!(
            totals.input_tokens, 20,
            "in-memory accumulation must work without a persist path"
        );
    }

    #[tokio::test]
    async fn main_usage_sink_record_tool_calls_persists_after_every_call() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        let path = layout.usage_totals_json();
        tokio::fs::create_dir_all(layout.memory_dir())
            .await
            .unwrap();
        let shared = Arc::new(Mutex::new(SessionUsageTotals::default()));
        let sink = MainUsageSink::new(Arc::clone(&shared), Some(&layout));

        let first = sink.record_tool_calls(3).await;
        assert_eq!(first.tool_calls, 3);
        let persisted_after_first = load_session_usage_totals(&path).await;
        assert_eq!(
            persisted_after_first, first,
            "every tool-call batch should write through to disk"
        );

        let second = sink.record_tool_calls(2).await;
        assert_eq!(
            second.tool_calls, 5,
            "tool call counts accumulate across batches on the shared Arc"
        );
        let persisted_after_second = load_session_usage_totals(&path).await;
        assert_eq!(persisted_after_second, second);
    }

    struct NamedProvider;

    #[async_trait]
    impl InferenceProvider for NamedProvider {
        async fn complete(
            &self,
            _messages: &[residuum_inference::Message],
            _tools: &[residuum_inference::ToolDefinition],
            _options: &residuum_inference::CompletionOptions,
        ) -> Result<InferenceResponse, InferenceError> {
            Ok(InferenceResponse::new(String::new(), vec![]))
        }

        fn model_name(&self) -> &'static str {
            "main-model"
        }
    }

    #[tokio::test]
    async fn main_usage_sink_records_each_call_as_main() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        let sink = MainUsageSink::new(
            Arc::new(Mutex::new(SessionUsageTotals::default())),
            Some(&layout),
        );
        let mut response = InferenceResponse::new("ok".to_string(), vec![]);
        response.usage = Some(usage(30, 4));
        sink.record_call(&NamedProvider, &Ok(response)).await;
        let failed = InferenceError::Attempted {
            error: Box::new(InferenceError::Timeout(5)),
            attempts: vec![residuum_inference::AttemptUsage {
                provider: "anthropic".to_string(),
                model: "main-model".to_string(),
                outcome: residuum_inference::AttemptOutcome::Failed,
                usage: None,
            }],
        };
        sink.record_call(&NamedProvider, &Err(failed)).await;

        let records = UsageRecorder::new(&layout)
            .load(
                chrono::DateTime::<chrono::Utc>::MIN_UTC,
                chrono::DateTime::<chrono::Utc>::MAX_UTC,
                None,
            )
            .await
            .unwrap();
        assert_eq!(records.len(), 2, "one record per call");
        let answered = records.first().unwrap();
        assert_eq!(answered.component, UsageComponent::Main);
        assert_eq!(answered.conversation, None);
        assert_eq!(answered.model, "main-model");
        assert_eq!(answered.usage, Some(usage(30, 4)));
        let failed_record = records.get(1).expect("the failed attempt is second");
        assert_eq!(
            failed_record.outcome,
            residuum_inference::AttemptOutcome::Failed
        );
    }
}
