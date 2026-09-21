//! Per-run session memory: threshold-based mid-run staging and the
//! completion pipeline that merges a finished run into global memory.
//!
//! Mirrors the main agent's observer flow (see `crate::gateway::memory`):
//! the run's own messages are checked against the same observer thresholds,
//! staged extractions are held locally (invisible to any other agent until
//! the run completes), and the run's full transcript is submitted to the
//! [`MemoryMergeWriter`] on completion.

use chrono_tz::Tz;

use crate::bus::HEARTBEAT_OK;
use crate::inference::Message;
use crate::memory::merge_writer::MemoryMergeWriter;
use crate::memory::observer::{ExtractedObservation, Extraction, ObserveAction, Observer};
use crate::memory::recent_messages::RecentMessage;
use crate::memory::tokens::estimate_message_tokens;
use crate::memory::types::{SourceTag, Visibility};
use crate::workspace::layout::WorkspaceLayout;

/// The subsystems a run's memory pipeline writes through, grouped because
/// both a live run's completion and startup recovery of an interrupted run
/// need the exact same set.
pub(crate) struct SessionMemoryEnv<'a> {
    pub(crate) observer: &'a Observer,
    pub(crate) merge_writer: &'a MemoryMergeWriter,
    pub(crate) layout: &'a WorkspaceLayout,
    pub(crate) episode_skip_token_floor: usize,
    pub(crate) tz: Tz,
}

/// A run's working memory: observations extracted mid-run but not yet
/// merged, plus how much of the transcript they already cover.
#[derive(Default)]
pub(crate) struct SessionMemory {
    /// Observations extracted so far but not yet merged.
    extracted: Vec<ExtractedObservation>,
    /// Narrative captured at the last mid-run extraction, if any.
    narrative: Option<String>,
    /// How many leading transcript messages `extracted`/`narrative` already
    /// cover — the completion pipeline only re-extracts the tail past this.
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
    /// completes.
    pub(crate) async fn maybe_stage(&mut self, transcript: &[Message], env: &SessionMemoryEnv<'_>) {
        let tail = transcript
            .get(self.covered_through.min(transcript.len())..)
            .unwrap_or_default();
        if tail.is_empty() {
            return;
        }
        let wrapped = wrap(tail, env.tz);
        if env.observer.check_thresholds(&wrapped) != ObserveAction::ForceNow {
            return;
        }
        match env.observer.extract(&wrapped, env.layout).await {
            Ok(extraction) => {
                tracing::info!(
                    observations = extraction.observations.len(),
                    "session run staged observations mid-run"
                );
                self.extracted.extend(extraction.observations);
                if extraction.narrative.is_some() {
                    self.narrative = extraction.narrative;
                }
                self.covered_through = transcript.len();
            }
            Err(e) => {
                tracing::warn!(error = %e, "session mid-run extraction failed, continuing unstaged");
            }
        }
    }
}

/// Wrap plain messages as `RecentMessage`s with `Background` visibility, for
/// the observer's extraction and threshold-check APIs.
fn wrap(messages: &[Message], tz: Tz) -> Vec<RecentMessage> {
    let now = crate::time::now_local(tz);
    messages
        .iter()
        .map(|m| RecentMessage {
            message: m.clone(),
            timestamp: now,
            visibility: Visibility::Background,
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
/// Takes the run's address/run id/category as a [`SourceTag`] rather than a
/// [`super::registry::SessionInfo`] so it can be reused both by a live run's
/// completion and by startup recovery, which only has a persisted
/// [`super::store::RunRecord`] to work from.
#[tracing::instrument(skip_all, fields(session.address = tag.session_address.as_deref().unwrap_or("unknown"), run.id = tag.run_id.as_deref().unwrap_or("unknown")))]
pub(crate) async fn complete_session_memory(
    tag: SourceTag,
    summary: &str,
    transcript: &[Message],
    memory: SessionMemory,
    env: &SessionMemoryEnv<'_>,
) -> Option<String> {
    let has_staged = memory.has_staged();
    let ended_with_heartbeat_ok = summary.contains(HEARTBEAT_OK);
    let total_tokens = estimate_message_tokens(transcript);
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

    let SessionMemory {
        extracted: mut observations,
        narrative: mut narrative_text,
        covered_through,
    } = memory;

    let tail = transcript
        .get(covered_through.min(transcript.len())..)
        .unwrap_or_default();
    if !tail.is_empty() {
        match env.observer.extract(&wrap(tail, env.tz), env.layout).await {
            Ok(extraction) => {
                observations.extend(extraction.observations);
                if extraction.narrative.is_some() {
                    narrative_text = extraction.narrative;
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "final session observation failed");
            }
        }
    }

    if observations.is_empty() {
        tracing::debug!("run produced no observations to merge");
        return None;
    }

    let extraction = Extraction {
        narrative: narrative_text,
        observations,
        messages: wrap(transcript, env.tz),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inference::Message;
    use crate::memory::observer::{Observer, ObserverConfig};
    use crate::memory::test_helpers::MockMemoryProvider;
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
            crate::memory::search::MemoryIndex::open_or_create(&layout.search_index_dir()).unwrap(),
        );
        let reflector = crate::memory::reflector::Reflector::disabled(chrono_tz::UTC);
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
            &transcript,
            SessionMemory::new(),
            &env,
        )
        .await;

        assert!(id.is_none(), "a HEARTBEAT_OK run should produce no episode");
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

        let id =
            complete_session_memory(sample_tag(), HEARTBEAT_OK, &transcript, memory, &env).await;

        assert!(
            id.is_some(),
            "a HEARTBEAT_OK run with staged observations must still merge"
        );
        let log = crate::memory::log_store::load_observation_log(&layout.observations_json())
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
            &transcript,
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
            &transcript,
            SessionMemory::new(),
            &env,
        )
        .await;

        assert!(id.is_some(), "a substantial run should merge an episode");
        let log = crate::memory::log_store::load_observation_log(&layout.observations_json())
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

        let id = complete_session_memory(sample_tag(), "hello", &transcript, memory, &env).await;

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
        memory.maybe_stage(&transcript, &env).await;

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
        memory.maybe_stage(&transcript, &env).await;

        assert!(
            !memory.has_staged(),
            "below threshold should not stage anything"
        );
        assert_eq!(memory.covered_through, 0);
    }
}
