//! Per-run session memory: threshold-based mid-run staging and the
//! completion pipeline that merges a finished run into global memory.
//!
//! Mirrors the main agent's observer flow (the gateway's `memory` module):
//! the run's own messages are checked against the same observer thresholds,
//! staged extractions are held locally (invisible to any other agent until
//! the run completes), and the run's full transcript is submitted to the
//! [`MemoryMergeWriter`] on completion.

use chrono::{DateTime, Utc};
use chrono_tz::Tz;

use residuum_bus::{HEARTBEAT_OK, ends_with_sentinel};
use residuum_config::WorkspaceLayout;
use residuum_inference::Message;
use residuum_memory::merge_writer::MemoryMergeWriter;
use residuum_memory::observer::{
    ExtractedObservation, Extraction, ObserveAction, Observer, extraction_tokens,
};
use residuum_memory::recent_messages::RecentMessage;
use residuum_memory::types::{SourceTag, Visibility};

/// The subsystems a run's memory pipeline writes through, grouped because
/// both a live run's completion and startup recovery of an interrupted run
/// need the exact same set.
pub struct SessionMemoryEnv<'a> {
    pub observer: &'a Observer,
    pub merge_writer: &'a MemoryMergeWriter,
    pub layout: &'a WorkspaceLayout,
    pub episode_skip_token_floor: usize,
    pub tz: Tz,
}

/// One message of a run's own transcript and when it was recorded. A
/// message read back from a transcript line written without a time has
/// none, and is stamped with the time the pipeline reads it.
#[derive(Clone, Copy)]
pub(crate) struct TimedMessage<'a> {
    pub(crate) message: &'a Message,
    pub(crate) recorded_at: Option<DateTime<Utc>>,
}

/// A run's working memory: observations extracted mid-run but not yet
/// merged, plus how much of the transcript they already cover.
#[derive(Default)]
pub(crate) struct SessionMemory {
    /// Observations extracted so far but not yet merged.
    extracted: Vec<ExtractedObservation>,
    /// Narrative captured at the last extraction, if any.
    narrative: Option<String>,
    /// How many leading transcript messages `extracted`/`narrative` already
    /// cover — later extractions only read the tail past this.
    covered_through: usize,
}

impl SessionMemory {
    /// Create empty working memory for a new run.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Whether anything has been staged yet.
    pub(crate) fn has_staged(&self) -> bool {
        !self.extracted.is_empty()
    }

    /// Check the run's transcript so far against the observer's thresholds
    /// and, if the force threshold is crossed, extract the unstaged tail
    /// into the staging area immediately — the same rotation the main agent
    /// performs when its own recent-message threshold is crossed. Staged
    /// observations are not visible to any other agent until the run
    /// completes. Each extraction's usage is recorded under `run_id`.
    pub(crate) async fn maybe_stage(
        &mut self,
        transcript: &[TimedMessage<'_>],
        run_id: &str,
        env: &SessionMemoryEnv<'_>,
    ) {
        let tail = wrap(self.unstaged(transcript), env.tz);
        if tail.is_empty() || env.observer.check_thresholds(&tail) != ObserveAction::ForceNow {
            return;
        }
        self.stage(&tail, Some(run_id), env).await;
    }

    /// The part of `items` (a run's transcript, wrapped or not) that nothing
    /// has been extracted from yet.
    fn unstaged<'a, T>(&self, items: &'a [T]) -> &'a [T] {
        items
            .get(self.covered_through.min(items.len())..)
            .unwrap_or_default()
    }

    /// Extract `tail` (the transcript past `covered_through`) into the
    /// staging area, in consecutive chunks that each fit the observer's
    /// force threshold, oldest first. Each chunk's observations are added
    /// and its narrative replaces the last one. Stops at the first chunk
    /// whose extraction fails, leaving it and everything after it unstaged:
    /// a later staging pass picks it up again, and a failure is usually the
    /// model being unreachable, which the remaining chunks would hit too.
    async fn stage(
        &mut self,
        tail: &[RecentMessage],
        conversation: Option<&str>,
        env: &SessionMemoryEnv<'_>,
    ) {
        let chunks = chunk_within(tail, env.observer.force_threshold_tokens());
        let chunk_count = chunks.len();
        for (index, chunk) in chunks.into_iter().enumerate() {
            match env.observer.extract(chunk, env.layout, conversation).await {
                Ok(extraction) => {
                    tracing::info!(
                        observations = extraction.observations.len(),
                        chunk = index + 1,
                        chunks = chunk_count,
                        messages = chunk.len(),
                        "session run extracted observations"
                    );
                    self.extracted.extend(extraction.observations);
                    if extraction.narrative.is_some() {
                        self.narrative = extraction.narrative;
                    }
                    self.covered_through += chunk.len();
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        chunk = index + 1,
                        chunks = chunk_count,
                        "session observation extraction failed, leaving the rest of the transcript unstaged"
                    );
                    return;
                }
            }
        }
    }
}

/// Split `messages` into consecutive chunks whose extraction tokens (see
/// [`extraction_tokens`]) stay within `budget`, so each chunk's observer
/// request stays near the budget. A single message larger than `budget` gets
/// a chunk of its own, since a message is never split or dropped. A
/// remainder of at most a quarter of the budget joins the chunk before it:
/// on its own it would cost an observer call, with the extraction prompt, for
/// a fragment.
fn chunk_within(messages: &[RecentMessage], budget: usize) -> Vec<&[RecentMessage]> {
    let mut chunks = Vec::new();
    let mut rest = messages;
    while !rest.is_empty() {
        let mut len = fitting_len(rest, budget);
        if within(rest.get(len..).unwrap_or_default(), budget / 4) {
            len = rest.len();
        }
        let (chunk, remainder) = rest.split_at(len);
        chunks.push(chunk);
        rest = remainder;
    }
    chunks
}

/// How many leading messages of `messages` fit within `budget` extraction
/// tokens. Always at least one, so a message over the budget still makes
/// progress.
fn fitting_len(messages: &[RecentMessage], budget: usize) -> usize {
    let mut tokens = 0_usize;
    for (i, rm) in messages.iter().enumerate() {
        tokens = tokens.saturating_add(extraction_tokens(rm));
        if i > 0 && tokens > budget {
            return i;
        }
    }
    messages.len()
}

/// Whether `messages` together come to at most `limit` extraction tokens.
fn within(messages: &[RecentMessage], limit: usize) -> bool {
    let mut tokens = 0_usize;
    for rm in messages {
        tokens = tokens.saturating_add(extraction_tokens(rm));
        if tokens > limit {
            return false;
        }
    }
    true
}

/// Wrap a run's messages as `RecentMessage`s with `Background` visibility,
/// for the observer's extraction and threshold-check APIs. Each keeps its
/// recorded time; one with none gets the current time.
fn wrap(messages: &[TimedMessage<'_>], tz: Tz) -> Vec<RecentMessage> {
    let now = residuum_core::time::now_local(tz);
    messages
        .iter()
        .map(|m| RecentMessage {
            message: m.message.clone(),
            timestamp: m
                .recorded_at
                .map_or(now, |at| at.with_timezone(&tz).naive_local()),
            visibility: Visibility::Background,
            turn_id: None,
        })
        .collect()
}

/// Run a finished run's completion memory pipeline: the skip check, a final
/// extraction over whatever the run staged nothing of yet, and the merge
/// into global memory. Returns the merged episode id, or `None` if the run
/// produced no episode — it staged nothing over the course of its run and
/// either ended with `HEARTBEAT_OK` or its transcript fell below the skip
/// token floor (its transcript is still kept in the session store either
/// way). Anything staged mid-run is merged regardless of how the run ended.
///
/// The final extraction goes through the same chunked staging as mid-run
/// staging, so each observer request carries about the force threshold's
/// worth of messages however long the unstaged tail is; a single message
/// larger than the threshold is sent on its own.
///
/// Takes the run's address/run id/category as a [`SourceTag`] rather than a
/// [`residuum_bus::session::SessionInfo`] so it can be reused both by a live run's
/// completion and by startup recovery, which only has a persisted
/// [`super::store::RunRecord`] to work from.
#[tracing::instrument(skip_all, fields(session.address = tag.session_address.as_deref().unwrap_or("unknown"), run.id = tag.run_id.as_deref().unwrap_or("unknown")))]
pub(crate) async fn complete_session_memory(
    tag: SourceTag,
    summary: &str,
    transcript: &[TimedMessage<'_>],
    mut memory: SessionMemory,
    env: &SessionMemoryEnv<'_>,
) -> Option<String> {
    let has_staged = memory.has_staged();
    let ended_with_heartbeat_ok = ends_with_sentinel(summary, HEARTBEAT_OK);
    let wrapped = wrap(transcript, env.tz);
    let total_tokens: usize = wrapped.iter().map(extraction_tokens).sum();
    let below_floor = total_tokens < env.episode_skip_token_floor;

    // A run with nothing staged skips producing an episode when it ended
    // quietly (HEARTBEAT_OK) or never accumulated enough content to be worth
    // one. Anything staged mid-run is real, already-extracted work — it
    // merges regardless of how the run ended, so a HEARTBEAT_OK finish never
    // discards it.
    if !has_staged && (ended_with_heartbeat_ok || below_floor) {
        tracing::debug!(
            has_staged,
            ended_with_heartbeat_ok,
            total_tokens,
            below_floor,
            "run produced no episode"
        );
        return None;
    }

    memory
        .stage(memory.unstaged(&wrapped), tag.run_id.as_deref(), env)
        .await;

    let SessionMemory {
        extracted: observations,
        narrative,
        ..
    } = memory;

    if observations.is_empty() {
        tracing::debug!("run produced no observations to merge");
        return None;
    }

    let extraction = Extraction {
        narrative,
        observations,
        messages: wrapped,
    };

    match env.merge_writer.merge(extraction, tag, env.tz).await {
        Ok(outcome) => {
            tracing::info!(episode_id = %outcome.id, "session run merged into global memory");
            Some(outcome.id)
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to merge session run into global memory");
            None
        }
    }
}

/// A transcript of messages with no recorded times, for tests that don't
/// check timestamps.
#[cfg(test)]
pub(crate) fn untimed(messages: &[Message]) -> Vec<TimedMessage<'_>> {
    messages
        .iter()
        .map(|message| TimedMessage {
            message,
            recorded_at: None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use residuum_inference::{ImageData, Message, ThinkingBlock};
    use residuum_memory::observer::{Observer, ObserverConfig};
    use residuum_memory::testing::MockMemoryProvider;
    use std::sync::Arc;

    fn sample_tag() -> SourceTag {
        SourceTag::session("spawned-researcher-0001", "run-test-1", "spawned")
    }

    const SAMPLE_RESPONSE: &str = r#"{
        "observations": [
            {"content": "found the answer", "timestamp": "2026-02-21T14:30", "visibility": "background"}
        ],
        "narrative": "the session researched the question and found an answer"
    }"#;

    fn observer_with_thresholds(soft: usize, force: usize) -> Observer {
        Observer::new(
            Box::new(MockMemoryProvider::new(SAMPLE_RESPONSE)),
            ObserverConfig {
                threshold_tokens: soft,
                force_threshold_tokens: force,
                ..ObserverConfig::default()
            },
        )
    }

    fn merge_writer(dir: &std::path::Path) -> MemoryMergeWriter {
        let layout = WorkspaceLayout::new(dir);
        let search_index = Arc::new(
            residuum_memory::search::MemoryIndex::open_or_create(&layout.search_index_dir())
                .unwrap(),
        );
        let reflector = residuum_memory::reflector::Reflector::disabled(chrono_tz::UTC);
        MemoryMergeWriter::new(reflector, layout, search_index, None, None)
    }

    #[tokio::test]
    async fn heartbeat_ok_run_produces_no_episode() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        let observer = observer_with_thresholds(100_000, 200_000);
        let mw = merge_writer(dir.path());
        let env = SessionMemoryEnv {
            observer: &observer,
            merge_writer: &mw,
            layout: &layout,
            episode_skip_token_floor: 2000,
            tz: chrono_tz::UTC,
        };
        let transcript = vec![
            Message::user("check things"),
            Message::assistant(HEARTBEAT_OK, None),
        ];

        let id = complete_session_memory(
            sample_tag(),
            HEARTBEAT_OK,
            &untimed(&transcript),
            SessionMemory::new(),
            &env,
        )
        .await;

        assert!(id.is_none(), "a HEARTBEAT_OK run should produce no episode");
    }

    #[tokio::test]
    async fn summary_merely_mentioning_heartbeat_ok_still_merges() {
        // A substantial reply that discusses the HEARTBEAT_OK sentinel in
        // prose (e.g. explaining that it deliberately did not end with one)
        // contains the literal text without meaning "nothing to report". A
        // plain substring check on the summary misdetects this the same way
        // a HEARTBEAT_OK ending does; only a summary that actually *ends*
        // with the sentinel should skip the episode.
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        let observer = observer_with_thresholds(100_000, 200_000);
        let mw = merge_writer(dir.path());
        let env = SessionMemoryEnv {
            observer: &observer,
            merge_writer: &mw,
            layout: &layout,
            // Isolate the heartbeat-ok check from the separate below-floor
            // skip: this transcript is short, so a non-trivial floor would
            // skip it for that reason regardless of the sentinel check.
            episode_skip_token_floor: 1,
            tz: chrono_tz::UTC,
        };
        let summary = "All steps completed. The instruction to omit HEARTBEAT_OK was honored \
             — this note ends without it, since the sentinel was not appropriate here."
            .to_string();
        let transcript = vec![
            Message::user("run the steps"),
            Message::assistant(summary.as_str(), None),
        ];

        let id = complete_session_memory(
            sample_tag(),
            &summary,
            &untimed(&transcript),
            SessionMemory::new(),
            &env,
        )
        .await;

        assert!(
            id.is_some(),
            "a substantial reply that merely mentions HEARTBEAT_OK mid-text must still merge"
        );
    }

    #[tokio::test]
    async fn heartbeat_ok_run_with_staged_observations_still_merges() {
        // A run that staged observations mid-run before ending quietly must
        // not have that work discarded — only a run with nothing staged
        // skips producing an episode on a HEARTBEAT_OK ending.
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        let observer = observer_with_thresholds(100_000, 200_000);
        let mw = merge_writer(dir.path());
        let env = SessionMemoryEnv {
            observer: &observer,
            merge_writer: &mw,
            layout: &layout,
            episode_skip_token_floor: 2000,
            tz: chrono_tz::UTC,
        };
        let transcript = vec![
            Message::user("check things"),
            Message::assistant(HEARTBEAT_OK, None),
        ];

        let mut memory = SessionMemory::new();
        memory.extracted.push(ExtractedObservation {
            timestamp: chrono::Utc::now().naive_utc(),
            visibility: Visibility::Background,
            content: "staged before the quiet ending".to_string(),
        });
        memory.covered_through = transcript.len();

        let id = complete_session_memory(
            sample_tag(),
            HEARTBEAT_OK,
            &untimed(&transcript),
            memory,
            &env,
        )
        .await;

        assert!(
            id.is_some(),
            "a HEARTBEAT_OK run with staged observations must still merge"
        );
        let log = residuum_memory::log_store::load_observation_log(&layout.observations_json())
            .await
            .unwrap();
        assert_eq!(
            log.observations.len(),
            1,
            "the staged observation must not be discarded"
        );
    }

    #[tokio::test]
    async fn tiny_run_below_floor_produces_no_episode() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        let observer = observer_with_thresholds(100_000, 200_000);
        let mw = merge_writer(dir.path());
        let env = SessionMemoryEnv {
            observer: &observer,
            merge_writer: &mw,
            layout: &layout,
            episode_skip_token_floor: 2000,
            tz: chrono_tz::UTC,
        };
        let transcript = vec![Message::user("hi"), Message::assistant("hello", None)];

        let id = complete_session_memory(
            sample_tag(),
            "hello",
            &untimed(&transcript),
            SessionMemory::new(),
            &env,
        )
        .await;

        assert!(
            id.is_none(),
            "a tiny run with nothing staged should produce no episode"
        );
    }

    #[tokio::test]
    async fn substantial_run_merges_an_episode() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        let observer = observer_with_thresholds(100_000, 200_000);
        let mw = merge_writer(dir.path());
        let env = SessionMemoryEnv {
            observer: &observer,
            merge_writer: &mw,
            layout: &layout,
            episode_skip_token_floor: 2000,
            tz: chrono_tz::UTC,
        };
        let big_content = "a".repeat(9000); // ~2250 tokens, above the 2000 floor
        let transcript = vec![
            Message::user("investigate the issue"),
            Message::assistant(big_content, None),
        ];

        let id = complete_session_memory(
            sample_tag(),
            "done",
            &untimed(&transcript),
            SessionMemory::new(),
            &env,
        )
        .await;

        assert!(id.is_some(), "a substantial run should merge an episode");
        let log = residuum_memory::log_store::load_observation_log(&layout.observations_json())
            .await
            .unwrap();
        assert_eq!(log.observations.len(), 1);
        assert!(log.observations.first().unwrap().source.is_session());
        assert_eq!(
            log.observations
                .first()
                .unwrap()
                .source
                .session_address
                .as_deref(),
            Some("spawned-researcher-0001")
        );
    }

    #[tokio::test]
    async fn tiny_run_with_staged_observations_still_merges() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        let observer = observer_with_thresholds(100_000, 200_000);
        let mw = merge_writer(dir.path());
        let env = SessionMemoryEnv {
            observer: &observer,
            merge_writer: &mw,
            layout: &layout,
            episode_skip_token_floor: 2000,
            tz: chrono_tz::UTC,
        };
        let transcript = vec![Message::user("hi"), Message::assistant("hello", None)];

        let mut memory = SessionMemory::new();
        memory.extracted.push(ExtractedObservation {
            timestamp: chrono::Utc::now().naive_utc(),
            visibility: Visibility::Background,
            content: "staged mid-run".to_string(),
        });
        memory.covered_through = transcript.len();

        let id =
            complete_session_memory(sample_tag(), "hello", &untimed(&transcript), memory, &env)
                .await;

        assert!(
            id.is_some(),
            "a tiny run must still merge when it has staged observations"
        );
    }

    #[tokio::test]
    async fn maybe_stage_extracts_and_marks_transcript_covered_above_force_threshold() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        let observer = observer_with_thresholds(10, 10);
        let mw = merge_writer(dir.path());
        let env = SessionMemoryEnv {
            observer: &observer,
            merge_writer: &mw,
            layout: &layout,
            episode_skip_token_floor: 2000,
            tz: chrono_tz::UTC,
        };
        let transcript = vec![Message::user(
            "a long investigation task with plenty of content to push past the tiny force threshold used in this test",
        )];

        let mut memory = SessionMemory::new();
        memory
            .maybe_stage(&untimed(&transcript), "run-test-1", &env)
            .await;

        assert!(
            memory.has_staged(),
            "crossing the force threshold should stage observations"
        );
        assert_eq!(memory.covered_through, transcript.len());
    }

    #[tokio::test]
    async fn maybe_stage_does_nothing_below_threshold() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        let observer = observer_with_thresholds(100_000, 200_000);
        let mw = merge_writer(dir.path());
        let env = SessionMemoryEnv {
            observer: &observer,
            merge_writer: &mw,
            layout: &layout,
            episode_skip_token_floor: 2000,
            tz: chrono_tz::UTC,
        };
        let transcript = vec![Message::user("hi")];

        let mut memory = SessionMemory::new();
        memory
            .maybe_stage(&untimed(&transcript), "run-test-1", &env)
            .await;

        assert!(
            !memory.has_staged(),
            "below threshold should not stage anything"
        );
        assert_eq!(memory.covered_through, 0);
    }

    /// Answers every observer call with [`SAMPLE_RESPONSE`] — or fails the
    /// call numbered `fail_on_call` (1-based) and every one after it — and
    /// keeps each call's prompt text, so a test can check what the observer
    /// was sent and in what order.
    struct RecordingProvider {
        calls: Arc<std::sync::Mutex<Vec<String>>>,
        fail_on_call: Option<usize>,
    }

    #[async_trait::async_trait]
    impl residuum_inference::InferenceProvider for RecordingProvider {
        async fn complete(
            &self,
            messages: &[Message],
            _tools: &[residuum_inference::ToolDefinition],
            _options: &residuum_inference::CompletionOptions,
        ) -> Result<residuum_inference::InferenceResponse, residuum_inference::InferenceError>
        {
            let prompt = messages
                .iter()
                .map(|m| m.content.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            let mut calls = self.calls.lock().unwrap();
            calls.push(prompt);
            if self.fail_on_call.is_some_and(|n| calls.len() >= n) {
                return Err(residuum_inference::InferenceError::Api(
                    "observer unavailable".to_string(),
                ));
            }
            Ok(residuum_inference::InferenceResponse::new(
                SAMPLE_RESPONSE.to_string(),
                vec![],
            ))
        }

        fn model_name(&self) -> &'static str {
            "recording"
        }
    }

    fn recording_observer(
        force: usize,
        fail_on_call: Option<usize>,
    ) -> (Observer, Arc<std::sync::Mutex<Vec<String>>>) {
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let observer = Observer::new(
            Box::new(RecordingProvider {
                calls: Arc::clone(&calls),
                fail_on_call,
            }),
            ObserverConfig {
                threshold_tokens: force,
                force_threshold_tokens: force,
                ..ObserverConfig::default()
            },
        );
        (observer, calls)
    }

    /// Six messages of about 47 extraction tokens each (a rendered entry of
    /// 185 characters), every one carrying a distinct marker so a test can
    /// tell which observer call saw it.
    fn marked_transcript() -> Vec<Message> {
        (0..6)
            .map(|i| Message::user(format!("<<m{i}>> {}", "x".repeat(123))))
            .collect()
    }

    #[tokio::test]
    async fn long_unstaged_tail_is_extracted_in_bounded_chunks_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        // Two marked messages fit under 100 tokens, three do not.
        let (observer, calls) = recording_observer(100, None);
        let mw = merge_writer(dir.path());
        let env = SessionMemoryEnv {
            observer: &observer,
            merge_writer: &mw,
            layout: &layout,
            episode_skip_token_floor: 1,
            tz: chrono_tz::UTC,
        };
        let transcript = marked_transcript();

        let id = complete_session_memory(
            sample_tag(),
            "done",
            &untimed(&transcript),
            SessionMemory::new(),
            &env,
        )
        .await;

        assert!(
            id.is_some(),
            "the chunked extraction should merge an episode"
        );
        let calls = calls.lock().unwrap().clone();
        assert_eq!(
            calls.len(),
            3,
            "six messages should take three bounded calls"
        );
        for (call, prompt) in calls.iter().enumerate() {
            for i in 0..6 {
                let marker = format!("<<m{i}>>");
                assert_eq!(
                    prompt.contains(&marker),
                    i / 2 == call,
                    "call {call} should see exactly messages {} and {}, in order",
                    call * 2,
                    call * 2 + 1
                );
            }
        }
        let log = residuum_memory::log_store::load_observation_log(&layout.observations_json())
            .await
            .unwrap();
        assert_eq!(
            log.observations.len(),
            3,
            "every chunk's observations should be merged"
        );
    }

    #[tokio::test]
    async fn completion_extracts_only_the_tail_past_what_was_staged() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        let (observer, calls) = recording_observer(100, None);
        let mw = merge_writer(dir.path());
        let env = SessionMemoryEnv {
            observer: &observer,
            merge_writer: &mw,
            layout: &layout,
            episode_skip_token_floor: 1,
            tz: chrono_tz::UTC,
        };
        let transcript = marked_transcript();
        let mut memory = SessionMemory::new();
        memory.extracted.push(ExtractedObservation {
            timestamp: chrono::Utc::now().naive_utc(),
            visibility: Visibility::Background,
            content: "staged mid-run".to_string(),
        });
        memory.covered_through = 4;

        let id = complete_session_memory(sample_tag(), "done", &untimed(&transcript), memory, &env)
            .await;

        assert!(id.is_some());
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "only the unstaged tail should be extracted");
        let prompt = calls.first().unwrap();
        assert!(prompt.contains("<<m4>>") && prompt.contains("<<m5>>"));
        assert!(
            !prompt.contains("<<m3>>"),
            "staged messages should not be sent again"
        );
    }

    #[tokio::test]
    async fn a_short_trailing_message_joins_the_chunk_before_it() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        let (observer, calls) = recording_observer(100, None);
        let mw = merge_writer(dir.path());
        let env = SessionMemoryEnv {
            observer: &observer,
            merge_writer: &mw,
            layout: &layout,
            episode_skip_token_floor: 1,
            tz: chrono_tz::UTC,
        };
        // The third message is what pushes the run over the threshold, on its
        // reasoning. The observer is sent only its short text, which is a
        // remainder small enough to ride with the two before it: one call.
        let mut transcript: Vec<Message> = marked_transcript().into_iter().take(2).collect();
        transcript.push(
            Message::assistant("<<m2>> ok", None)
                .with_thinking(vec![ThinkingBlock::text("r".repeat(300))]),
        );

        let mut memory = SessionMemory::new();
        memory
            .maybe_stage(&untimed(&transcript), "run-test-1", &env)
            .await;

        assert_eq!(memory.covered_through, 3, "the whole tail should be staged");
        let calls = calls.lock().unwrap();
        assert_eq!(
            calls.len(),
            1,
            "a short trailing message should not cost a second observer call"
        );
        let prompt = calls.first().unwrap();
        assert!(prompt.contains("<<m2>> ok"));
        assert!(
            !prompt.contains("rrrr"),
            "reasoning text should not be sent to the observer"
        );
    }

    #[tokio::test]
    async fn a_failed_chunk_stops_staging_and_leaves_the_rest_unstaged() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        let (observer, calls) = recording_observer(100, Some(2));
        let mw = merge_writer(dir.path());
        let env = SessionMemoryEnv {
            observer: &observer,
            merge_writer: &mw,
            layout: &layout,
            episode_skip_token_floor: 1,
            tz: chrono_tz::UTC,
        };
        let transcript = marked_transcript();

        let mut memory = SessionMemory::new();
        memory
            .maybe_stage(&untimed(&transcript), "run-test-1", &env)
            .await;

        assert_eq!(
            calls.lock().unwrap().len(),
            2,
            "staging should stop at the failed chunk"
        );
        assert!(
            memory.has_staged(),
            "the first chunk's observations stay staged"
        );
        assert_eq!(
            memory.covered_through, 2,
            "only the chunk that succeeded counts as covered"
        );
    }

    #[test]
    fn chunks_stay_within_budget_and_keep_every_message_in_order() {
        let now = chrono::Utc::now();
        let mut messages: Vec<Message> = marked_transcript();
        // One message alone over budget still gets extracted, on its own.
        messages.insert(3, Message::user("y".repeat(1000)));
        let timed: Vec<TimedMessage<'_>> = messages
            .iter()
            .map(|message| TimedMessage {
                message,
                recorded_at: Some(now),
            })
            .collect();
        let wrapped = wrap(&timed, chrono_tz::UTC);

        let chunks = chunk_within(&wrapped, 100);

        let rejoined: Vec<&str> = chunks
            .iter()
            .flat_map(|chunk| chunk.iter().map(|rm| rm.message.content.as_str()))
            .collect();
        let original: Vec<&str> = messages.iter().map(|m| m.content.as_str()).collect();
        assert_eq!(
            rejoined, original,
            "chunks must keep every message, in order"
        );
        for chunk in &chunks {
            let tokens: usize = chunk.iter().map(extraction_tokens).sum();
            assert!(
                tokens <= 100 + 100 / 4 || chunk.len() == 1,
                "a chunk of {} messages estimated at {tokens} tokens is over budget",
                chunk.len()
            );
        }
        assert!(
            chunks.iter().any(|chunk| chunk.len() == 1
                && chunk
                    .first()
                    .is_some_and(|rm| rm.message.content.starts_with('y'))),
            "the oversized message should be a chunk of its own"
        );
    }

    #[tokio::test]
    async fn recorded_message_times_reach_the_observer() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        let (observer, calls) = recording_observer(100_000, None);
        let mw = merge_writer(dir.path());
        let tz = chrono_tz::America::New_York;
        let env = SessionMemoryEnv {
            observer: &observer,
            merge_writer: &mw,
            layout: &layout,
            episode_skip_token_floor: 1,
            tz,
        };
        let first = Message::user("start the investigation");
        let second = Message::assistant("found it", None);
        let at = |s: &str| {
            chrono::DateTime::parse_from_rfc3339(s)
                .unwrap()
                .with_timezone(&chrono::Utc)
        };
        let transcript = [
            TimedMessage {
                message: &first,
                recorded_at: Some(at("2026-03-01T09:15:00Z")),
            },
            TimedMessage {
                message: &second,
                recorded_at: Some(at("2026-03-01T10:45:00Z")),
            },
        ];

        let id = complete_session_memory(
            sample_tag(),
            "found it",
            &transcript,
            SessionMemory::new(),
            &env,
        )
        .await;

        assert!(id.is_some());
        let calls = calls.lock().unwrap();
        let prompt = calls.first().unwrap();
        // 09:15 and 10:45 UTC are 04:15 and 05:45 in New York (EST).
        assert!(
            prompt.contains("[2026-03-01T04:15:00] [user]"),
            "the first message should carry its own recorded time: {prompt}"
        );
        assert!(
            prompt.contains("[2026-03-01T05:45:00] [assistant]"),
            "the second message should carry its own recorded time: {prompt}"
        );
    }

    #[tokio::test]
    async fn reasoning_text_does_not_shrink_an_observer_chunk() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        let (observer, calls) = recording_observer(100, None);
        let mw = merge_writer(dir.path());
        let env = SessionMemoryEnv {
            observer: &observer,
            merge_writer: &mw,
            layout: &layout,
            episode_skip_token_floor: 1,
            tz: chrono_tz::UTC,
        };
        // Each message's reasoning alone is over the budget. The observer is
        // never sent it, so both short messages still fit one request.
        let transcript: Vec<Message> = marked_transcript()
            .into_iter()
            .take(2)
            .map(|m| m.with_thinking(vec![ThinkingBlock::text("reasoning-only ".repeat(400))]))
            .collect();

        let id = complete_session_memory(
            sample_tag(),
            "done",
            &untimed(&transcript),
            SessionMemory::new(),
            &env,
        )
        .await;

        assert!(id.is_some());
        let calls = calls.lock().unwrap();
        assert_eq!(
            calls.len(),
            1,
            "reasoning text should not split two short messages into two requests"
        );
        assert!(
            !calls.first().unwrap().contains("reasoning-only"),
            "reasoning text should not be sent to the observer"
        );
    }

    #[tokio::test]
    async fn reasoning_and_images_alone_do_not_clear_the_skip_floor() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        let (observer, calls) = recording_observer(100_000, None);
        let mw = merge_writer(dir.path());
        let env = SessionMemoryEnv {
            observer: &observer,
            merge_writer: &mw,
            layout: &layout,
            episode_skip_token_floor: 2000,
            tz: chrono_tz::UTC,
        };
        // About 2,600 tokens by the threshold estimate, which counts the
        // reasoning and the image. The observer receives neither.
        let image = ImageData {
            media_type: "image/png".to_string(),
            data: "iVBORw0KGgo=".to_string(),
        };
        let mut with_image = Message::user("hi");
        with_image.images = vec![image];
        let transcript = vec![
            with_image,
            Message::assistant("hello", None)
                .with_thinking(vec![ThinkingBlock::text("r".repeat(4000))]),
        ];

        let id = complete_session_memory(
            sample_tag(),
            "hello",
            &untimed(&transcript),
            SessionMemory::new(),
            &env,
        )
        .await;

        assert!(
            id.is_none(),
            "a run whose only bulk is reasoning and images should be skipped"
        );
        assert!(calls.lock().unwrap().is_empty());
    }

    #[test]
    fn a_message_without_a_recorded_time_gets_the_current_time() {
        let message = Message::user("from an old transcript line");
        let before = residuum_core::time::now_local(chrono_tz::UTC);
        let wrapped = wrap(
            &[TimedMessage {
                message: &message,
                recorded_at: None,
            }],
            chrono_tz::UTC,
        );
        let after = residuum_core::time::now_local(chrono_tz::UTC);

        let stamped = wrapped.first().unwrap().timestamp;
        assert!(
            stamped >= before && stamped <= after,
            "an untimed message should be stamped when it is read"
        );
    }
}
