//! Memory merge writer: the single writer for global memory.
//!
//! Episode id allocation, the observation log append, the episode transcript
//! and its search-chunk index, embedding, and the reflector trigger all go
//! through [`MemoryMergeWriter`]. The main agent's own observation flow and
//! every session run's completion pipeline call the same writer, so episode
//! numbering and log appends never race between concurrent runs.
//!
//! Two locks divide the work: `merge_lock` serializes episode id allocation
//! and the durable per-episode writes (transcript, observation archives,
//! chunk index, merged-run record, completion marker), and is released
//! before indexing, embedding, or the reflector run — so a slow network or
//! LLM call there never blocks a concurrent merge's episode id allocation.
//! `log_lock` separately serializes the global observation log itself: a
//! merge's append and the reflector's compress-and-replace rewrite of that
//! file must never interleave.
//!
//! A session run's completion pipeline can run more than once for the same
//! run id — a crash between a successful merge and the run record being
//! marked completed leaves the run `running`, and startup recovery re-runs
//! the pipeline from the same transcript. `merge` refuses to create a
//! duplicate episode for a run id already recorded in
//! [`crate::memory::merged_run_log`].

use std::sync::Arc;

use anyhow::Context;
use chrono_tz::Tz;
use tokio::sync::Mutex;

use crate::inference::EmbeddingProvider;
use crate::memory::chunk_extractor::{extract_chunks, write_idx_jsonl};
use crate::memory::episode_store::{
    episode_idx_path, episode_obs_path, next_episode_id, write_completion_marker,
    write_episode_transcript_tagged,
};
use crate::memory::log_store::{append_observations, save_episode_observations};
use crate::memory::observer::Extraction;
use crate::memory::reflector::{Reflector, ReflectorConfig};
use crate::memory::search::MemoryIndex;
use crate::memory::types::{
    Episode, IndexChunk, IndexManifest, ManifestFileEntry, Observation, SourceTag,
};
use crate::memory::vector_store::VectorStore;
use crate::time::now_local;
use crate::workspace::layout::WorkspaceLayout;

/// The outcome of a successful merge: what would previously have been
/// returned by `Observer::observe`, plus whether the reflector fired.
pub struct MergeOutcome {
    /// The episode identifier (e.g., `"ep-001"`).
    pub id: String,
    /// Path to the transcript file on disk.
    pub transcript_path: std::path::PathBuf,
    /// Narrative summary captured at merge time, if any.
    pub narrative: Option<String>,
    /// The merged observations, tagged with `tag`.
    pub observations: Vec<Observation>,
    /// Interaction-pair chunks extracted from the transcript.
    pub chunks: Vec<IndexChunk>,
    /// Episode date in `YYYY-MM-DD` format.
    pub date: String,
    /// Whether the reflector ran as part of this merge.
    pub reflected: bool,
}

/// State that is mutated on a config/provider reload: the reflector and the
/// embedding provider. Kept together so a reload's writes and a concurrent
/// merge's reads never interleave. Locked only around the
/// indexing/embedding/reflection phase of a merge — never around episode id
/// allocation or the durable writes in [`MemoryMergeWriter::persist_episode`]
/// — so a slow embedding call or reflector LLM call never blocks a
/// concurrent merge from allocating its episode id.
struct MergeState {
    reflector: Reflector,
    embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
}

/// The single serialized writer for global memory.
///
/// Shared (via `Arc`) between the main agent's event loop and every
/// session's completion pipeline, so this is the one place episode ids are
/// allocated, the observation log is appended, and the reflector is checked.
pub struct MemoryMergeWriter {
    /// Serializes episode id allocation and the durable per-episode writes
    /// in [`Self::persist_episode`] (transcript, observation archives, chunk
    /// index, merged-run record, completion marker), so episode numbering
    /// never races. Released before indexing, embedding, or reflection run.
    merge_lock: Mutex<()>,
    /// Serializes writes to the global observation log: a merge's append
    /// (inside `persist_episode`) and the reflector's compress-and-replace
    /// rewrite of that same file must never interleave.
    log_lock: Mutex<()>,
    state: Mutex<MergeState>,
    layout: WorkspaceLayout,
    search_index: Arc<MemoryIndex>,
    vector_store: Option<Arc<VectorStore>>,
}

impl MemoryMergeWriter {
    /// Create a new merge writer over the given subsystems.
    #[must_use]
    pub fn new(
        reflector: Reflector,
        layout: WorkspaceLayout,
        search_index: Arc<MemoryIndex>,
        vector_store: Option<Arc<VectorStore>>,
        embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
    ) -> Self {
        Self {
            merge_lock: Mutex::new(()),
            log_lock: Mutex::new(()),
            state: Mutex::new(MergeState {
                reflector,
                embedding_provider,
            }),
            layout,
            search_index,
            vector_store,
        }
    }

    /// Update the reflector's configuration (e.g. after a config reload).
    pub async fn update_reflector_config(&self, config: ReflectorConfig) {
        self.state.lock().await.reflector.update_config(config);
    }

    /// Replace the reflector wholesale (e.g. after a provider reload rebuilds
    /// it with a fresh provider chain). A subsequent
    /// [`Self::update_reflector_config`] call is still safe — reloads apply
    /// providers and thresholds in a fixed order regardless of which changed,
    /// so the two calls simply agree on the same end state.
    pub async fn swap_reflector(&self, reflector: Reflector) {
        self.state.lock().await.reflector = reflector;
    }

    /// Replace the embedding provider (e.g. after a provider reload).
    pub async fn set_embedding_provider(
        &self,
        embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
    ) {
        self.state.lock().await.embedding_provider = embedding_provider;
    }

    /// Force a reflection cycle regardless of the observation log's size.
    ///
    /// # Errors
    /// Returns an error if the LLM call fails or file persistence fails.
    pub async fn force_reflect(&self) -> anyhow::Result<crate::memory::types::ObservationLog> {
        let state = self.state.lock().await;
        let _log_guard = self.log_lock.lock().await;
        state.reflector.reflect(&self.layout).await
    }

    /// Whether `tag` names a session run that has already been durably
    /// merged into an episode, per the [`crate::memory::merged_run_log`]
    /// record. Always `Ok(None)` for the main agent's own merges
    /// (`SourceTag::main` carries no run id).
    ///
    /// Checked before allocating a new episode so a re-run of a run's
    /// completion pipeline — e.g. startup recovery after a crash between a
    /// successful merge and the run record being marked completed — refuses
    /// to create a duplicate episode instead of merging the same transcript
    /// twice.
    ///
    /// # Errors
    /// Returns an error if the merged-run record exists but cannot be read.
    async fn check_already_merged(&self, tag: &SourceTag) -> anyhow::Result<Option<MergeOutcome>> {
        let Some(run_id) = tag.run_id.as_deref() else {
            return Ok(None);
        };
        let merged =
            crate::memory::merged_run_log::load_merged_runs(&self.layout.merged_runs_json())
                .await?;
        let Some(episode_id) = merged.get(run_id).cloned() else {
            return Ok(None);
        };
        tracing::info!(run_id, episode_id = %episode_id, "run already merged into global memory; refusing duplicate merge");
        let transcript_path = crate::memory::episode_store::find_episode_path(
            &self.layout.episodes_dir(),
            &episode_id,
        )?
        .unwrap_or_else(|| {
            self.layout
                .episodes_dir()
                .join(format!("{episode_id}.jsonl"))
        });
        Ok(Some(MergeOutcome {
            id: episode_id,
            transcript_path,
            narrative: None,
            observations: Vec::new(),
            chunks: Vec::new(),
            date: String::new(),
            reflected: false,
        }))
    }

    /// Look up the episode a session run id has already been merged into, if
    /// any, without going through the full [`Self::merge`] pipeline. Used by
    /// startup recovery to backfill a recovered run's record instead of
    /// re-running the completion pipeline for a run that already merged
    /// before the crash.
    ///
    /// # Errors
    /// Returns an error if the merged-run record exists but cannot be read.
    pub(crate) async fn find_merged_episode(&self, run_id: &str) -> anyhow::Result<Option<String>> {
        Ok(
            crate::memory::merged_run_log::load_merged_runs(&self.layout.merged_runs_json())
                .await?
                .get(run_id)
                .cloned(),
        )
    }

    /// Merge an extraction into global memory: allocate an episode id, write
    /// the transcript and observation archives, index and embed, and check
    /// whether the reflector should run.
    ///
    /// Episode id allocation and the durable per-episode writes in
    /// [`Self::persist_episode`] are serialized against every other merge
    /// (main agent or session) through the writer's merge lock, so episode
    /// numbering and log appends never race. That lock is released before
    /// indexing, embedding, and the reflector check run, so a slow network
    /// or LLM call there never blocks a concurrent merge's episode id
    /// allocation.
    ///
    /// # Errors
    /// Returns an error if the episode id cannot be allocated or the
    /// transcript, observation log, chunk index, merged-run record, or
    /// completion marker cannot be written — these are the durable record a
    /// merge must not silently lose. Indexing, embedding, and reflector
    /// failures are logged as warnings and never fail the merge.
    #[tracing::instrument(skip_all, fields(session_address = tag.session_address.as_deref().unwrap_or("main")))]
    pub async fn merge(
        &self,
        extraction: Extraction,
        tag: SourceTag,
        tz: Tz,
    ) -> anyhow::Result<MergeOutcome> {
        if let Some(outcome) = self.check_already_merged(&tag).await? {
            return Ok(outcome);
        }

        let merge_guard = self.merge_lock.lock().await;
        // Re-check under the lock: closes the window between the check above
        // and another merge for the same run id finishing while this call
        // waited for the lock.
        if let Some(outcome) = self.check_already_merged(&tag).await? {
            return Ok(outcome);
        }

        let episode_id = next_episode_id(&self.layout.episodes_dir())
            .await
            .context("failed to allocate episode id")?;
        let episode = Episode {
            id: episode_id,
            date: now_local(tz).date(),
            observations: extraction
                .observations
                .iter()
                .map(|e| e.content.clone())
                .collect(),
        };

        let (transcript_path, observations, chunks) =
            self.persist_episode(&episode, &extraction, &tag).await?;
        drop(merge_guard);

        let date_str = episode.date.to_string();
        let reflected = {
            let state = self.state.lock().await;
            self.index_embed_and_reflect(&state, &episode.id, &date_str, &observations, &chunks)
                .await
        };

        tracing::info!(
            episode_id = %episode.id,
            observations = observations.len(),
            chunks = chunks.len(),
            reflected,
            "episode merged"
        );

        Ok(MergeOutcome {
            id: episode.id,
            transcript_path,
            narrative: extraction.narrative,
            observations,
            chunks,
            date: date_str,
            reflected,
        })
    }

    /// Write the episode's durable artifacts: transcript, per-episode and
    /// global observation archives, the interaction-pair chunk index, the
    /// merged-run record (for a session tag), and the completion marker —
    /// in that order, with the marker last. Returns the transcript path,
    /// the tagged observations, and the extracted chunks for the caller to
    /// index.
    ///
    /// Called only while [`Self::merge`] holds `merge_lock`.
    ///
    /// # Errors
    /// Returns an error, propagated from whichever step failed, if any
    /// artifact cannot be written. Every step here is required: the
    /// completion marker's presence certifies that all of them landed on
    /// disk, so a failure anywhere leaves the marker absent — the signal
    /// [`crate::memory::episode_store::find_interrupted_episodes`] uses to
    /// detect an interrupted write.
    async fn persist_episode(
        &self,
        episode: &Episode,
        extraction: &Extraction,
        tag: &SourceTag,
    ) -> anyhow::Result<(std::path::PathBuf, Vec<Observation>, Vec<IndexChunk>)> {
        let transcript_path =
            crate::memory::episode_store::episode_jsonl_path(&self.layout.episodes_dir(), episode);
        let messages: Vec<crate::inference::Message> = extraction
            .messages
            .iter()
            .map(|rm| rm.message.clone())
            .collect();
        write_episode_transcript_tagged(
            &self.layout.episodes_dir(),
            episode,
            &messages,
            tag,
            extraction.narrative.as_deref(),
        )
        .await
        .context("failed to write episode transcript")?;
        tracing::debug!(episode_id = %episode.id, "episode transcript written");

        let observations: Vec<Observation> = extraction
            .observations
            .iter()
            .map(|e| Observation {
                timestamp: e.timestamp,
                source_episodes: Some(episode.id.clone()),
                visibility: e.visibility.clone(),
                content: e.content.clone(),
                source: tag.clone(),
            })
            .collect();

        let obs_path = episode_obs_path(&self.layout.episodes_dir(), episode);
        save_episode_observations(&obs_path, &observations)
            .await
            .context("failed to write per-episode observation archive")?;

        {
            // Serialized against the reflector's compress-and-replace
            // rewrite of the same file, via `log_lock` — never appended to
            // while a reflection is reading and rewriting it.
            let _log_guard = self.log_lock.lock().await;
            append_observations(&self.layout.observations_json(), observations.clone())
                .await
                .context("failed to append to global observation log")?;
        }
        tracing::debug!(episode_id = %episode.id, count = observations.len(), "global observations updated");

        let date_str = episode.date.to_string();
        let chunks = extract_chunks(&extraction.messages, &episode.id, &date_str, 2);
        let idx_path = episode_idx_path(&self.layout.episodes_dir(), episode);
        write_idx_jsonl(&idx_path, &chunks)
            .await
            .context("failed to write interaction-pair chunk index")?;

        if let Some(run_id) = tag.run_id.as_deref() {
            crate::memory::merged_run_log::record_merged_run(
                &self.layout.merged_runs_json(),
                run_id,
                &episode.id,
            )
            .await
            .context("failed to record merged run id")?;
        }

        write_completion_marker(&self.layout.episodes_dir(), episode)
            .await
            .context("failed to write episode completion marker")?;

        Ok((transcript_path, observations, chunks))
    }

    /// Index and embed the episode's observations and chunks, record the
    /// manifest entries, and check whether the reflector should run.
    /// Returns whether the reflector fired. Never fails the merge — every
    /// failure here is logged and degrades search/reflection, not the
    /// durable record.
    async fn index_embed_and_reflect(
        &self,
        state: &MergeState,
        episode_id: &str,
        date_str: &str,
        observations: &[Observation],
        chunks: &[IndexChunk],
    ) -> bool {
        let obs_ids = self
            .search_index
            .index_observations(episode_id, date_str, observations)
            .inspect_err(|e| tracing::warn!(error = %e, episode_id, "failed to index observations; startup sync will retry"))
            .ok();
        let chunk_ids = self
            .search_index
            .index_chunks(chunks)
            .inspect_err(|e| tracing::warn!(error = %e, episode_id, "failed to index chunks; startup sync will retry"))
            .ok();

        let embedded = embed_and_insert(
            self.vector_store.as_deref(),
            state.embedding_provider.as_deref(),
            episode_id,
            date_str,
            observations,
            chunks,
        )
        .await;

        record_episode_in_manifest(
            &self.layout,
            episode_id,
            date_str,
            obs_ids,
            chunk_ids,
            embedded,
        )
        .await;

        let log = crate::memory::log_store::load_observation_log(&self.layout.observations_json())
            .await
            .unwrap_or_default();
        if !state.reflector.should_reflect(&log) {
            return false;
        }
        // Serialized against a concurrent merge's observation-log append via
        // `log_lock` — the reflector reads and rewrites the whole file, so it
        // must never interleave with an append.
        let _log_guard = self.log_lock.lock().await;
        match state.reflector.reflect(&self.layout).await {
            Ok(compressed) => {
                tracing::info!(
                    episodes = compressed.observations.len(),
                    "reflector compressed observation log"
                );
                true
            }
            Err(e) => {
                tracing::warn!(error = %e, "reflector failed");
                false
            }
        }
    }
}

/// Embed observations and chunks into the vector store, if both an embedding
/// provider and a vector store are configured. Returns whether embedding
/// succeeded for every non-empty batch (used to record manifest state).
async fn embed_and_insert(
    vector_store: Option<&VectorStore>,
    embedding_provider: Option<&dyn EmbeddingProvider>,
    episode_id: &str,
    date: &str,
    observations: &[Observation],
    chunks: &[IndexChunk],
) -> bool {
    let (Some(vs), Some(ep)) = (vector_store, embedding_provider) else {
        return false;
    };

    let mut all_ok = true;

    if !observations.is_empty() {
        let texts: Vec<&str> = observations.iter().map(|o| o.content.as_str()).collect();
        match ep.embed(&texts).await {
            Ok(response) => {
                if let Err(e) =
                    vs.insert_observations(episode_id, date, observations, &response.embeddings)
                {
                    tracing::warn!(error = %e, "failed to insert observation vectors");
                    all_ok = false;
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "failed to embed observations");
                all_ok = false;
            }
        }
    }

    if !chunks.is_empty() {
        let texts: Vec<&str> = chunks.iter().map(|c| c.content.as_str()).collect();
        match ep.embed(&texts).await {
            Ok(response) => {
                if let Err(e) = vs.insert_chunks(chunks, &response.embeddings) {
                    tracing::warn!(error = %e, "failed to insert chunk vectors");
                    all_ok = false;
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "failed to embed chunks");
                all_ok = false;
            }
        }
    }

    all_ok
}

/// Date-formatted subdirectory (`YYYY-MM/DD`) for an episode's date string.
fn episode_date_dir(date: &str) -> Option<String> {
    let year_month = date.get(..7)?;
    let day = date.get(8..10)?;
    Some(format!("{year_month}/{day}"))
}

/// Record a just-indexed episode's `.obs.json` and `.idx.jsonl` in the manifest.
async fn record_episode_in_manifest(
    layout: &WorkspaceLayout,
    episode_id: &str,
    date: &str,
    obs_ids: Option<Vec<String>>,
    chunk_ids: Option<Vec<String>>,
    embedded: bool,
) {
    let manifest_path = layout.index_manifest_json();
    let mut manifest = match IndexManifest::load(&manifest_path).await {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(error = %e, episode_id, "failed to load index manifest to record episode; startup sync will reindex it");
            return;
        }
    };

    let Some(date_dir) = episode_date_dir(date) else {
        tracing::warn!(date, "invalid date format in episode result");
        return;
    };

    let files = [
        (
            format!("episodes/{date_dir}/{episode_id}.obs.json"),
            obs_ids,
        ),
        (
            format!("episodes/{date_dir}/{episode_id}.idx.jsonl"),
            chunk_ids,
        ),
    ];

    let memory_dir = layout.memory_dir();

    for (rel_path, doc_ids) in files {
        let Some(doc_ids) = doc_ids else {
            continue;
        };
        let abs_path = memory_dir.join(&rel_path);
        let modified = match std::fs::metadata(&abs_path).and_then(|m| m.modified()) {
            Ok(modified) => modified,
            Err(e) => {
                tracing::warn!(error = %e, path = %abs_path.display(), "failed to read mtime of indexed episode file; startup sync will reindex it");
                continue;
            }
        };
        let dt: chrono::DateTime<chrono::Utc> = modified.into();
        manifest.files.insert(
            rel_path,
            ManifestFileEntry {
                mtime: dt.format("%Y-%m-%dT%H:%M:%S").to_string(),
                doc_ids,
                embedded,
            },
        );
    }

    if let Err(e) = manifest.save(&manifest_path).await {
        tracing::warn!(error = %e, episode_id, "failed to save index manifest after recording episode");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inference::Message;
    use crate::memory::observer::ExtractedObservation;
    use crate::memory::recent_messages::RecentMessage;
    use crate::memory::reflector::ReflectorConfig;
    use crate::memory::types::Visibility;
    use std::path::Path;

    fn writer(dir: &Path) -> MemoryMergeWriter {
        let layout = WorkspaceLayout::new(dir);
        let search_index =
            Arc::new(MemoryIndex::open_or_create(&layout.search_index_dir()).unwrap());
        let reflector = Reflector::new(
            Box::new(crate::inference::providers::null::NullProvider),
            ReflectorConfig {
                threshold_tokens: usize::MAX,
                ..ReflectorConfig::default()
            },
        );
        MemoryMergeWriter::new(reflector, layout, search_index, None, None)
    }

    fn sample_extraction(content: &str) -> Extraction {
        Extraction {
            narrative: Some("we were talking about testing".to_string()),
            observations: vec![ExtractedObservation {
                timestamp: chrono::Utc::now().naive_utc(),
                visibility: Visibility::User,
                content: content.to_string(),
            }],
            messages: vec![RecentMessage {
                message: Message::user(content),
                timestamp: chrono::Utc::now().naive_utc(),
                visibility: Visibility::User,
            }],
        }
    }

    #[tokio::test]
    async fn merge_writes_transcript_and_appends_observations() {
        let dir = tempfile::tempdir().unwrap();
        let mw = writer(dir.path());

        let outcome = mw
            .merge(
                sample_extraction("hello"),
                SourceTag::main(),
                chrono_tz::UTC,
            )
            .await
            .unwrap();

        assert_eq!(outcome.id, "ep-001");
        assert!(outcome.transcript_path.exists());
        assert_eq!(outcome.observations.len(), 1);
        assert!(!outcome.observations.first().unwrap().source.is_session());

        let layout = WorkspaceLayout::new(dir.path());
        let log = crate::memory::log_store::load_observation_log(&layout.observations_json())
            .await
            .unwrap();
        assert_eq!(log.observations.len(), 1);
    }

    #[tokio::test]
    async fn merge_tags_session_observations_and_meta() {
        let dir = tempfile::tempdir().unwrap();
        let mw = writer(dir.path());
        let tag = SourceTag::session("spawned-researcher-0001", "run-1", "spawned");

        let outcome = mw
            .merge(
                sample_extraction("found something"),
                tag.clone(),
                chrono_tz::UTC,
            )
            .await
            .unwrap();

        assert!(outcome.observations.first().unwrap().source.is_session());
        assert_eq!(
            outcome
                .observations
                .first()
                .unwrap()
                .source
                .session_address
                .as_deref(),
            Some("spawned-researcher-0001")
        );

        let (meta, _) = crate::memory::episode_store::read_episode_jsonl(&outcome.transcript_path)
            .await
            .unwrap();
        assert_eq!(meta.source, tag);
        assert_eq!(
            meta.narrative.as_deref(),
            Some("we were talking about testing")
        );
    }

    #[tokio::test]
    async fn concurrent_merges_allocate_unique_sequential_episode_ids() {
        let dir = tempfile::tempdir().unwrap();
        let mw = Arc::new(writer(dir.path()));

        let mut handles = Vec::new();
        for i in 0..5 {
            let mw = Arc::clone(&mw);
            handles.push(tokio::spawn(async move {
                mw.merge(
                    sample_extraction(&format!("observation {i}")),
                    SourceTag::main(),
                    chrono_tz::UTC,
                )
                .await
                .unwrap()
            }));
        }

        let mut ids = Vec::new();
        for h in handles {
            ids.push(h.await.unwrap().id);
        }
        ids.sort();
        ids.dedup();
        assert_eq!(
            ids.len(),
            5,
            "every concurrent merge should get a unique episode id"
        );

        let layout = WorkspaceLayout::new(dir.path());
        let log = crate::memory::log_store::load_observation_log(&layout.observations_json())
            .await
            .unwrap();
        assert_eq!(
            log.observations.len(),
            5,
            "the observation log should have exactly one entry per merge, never lost to a race"
        );
    }

    #[tokio::test]
    async fn force_reflect_updates_observation_log() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        tokio::fs::create_dir_all(layout.memory_dir())
            .await
            .unwrap();
        let mut log = crate::memory::types::ObservationLog::new();
        log.observations.push(crate::memory::types::Observation {
            timestamp: chrono::Utc::now().naive_utc(),
            source_episodes: Some("ep-001".to_string()),
            visibility: Visibility::User,
            content: "an observation".to_string(),
            source: SourceTag::main(),
        });
        crate::memory::log_store::save_observation_log(&layout.observations_json(), &log)
            .await
            .unwrap();

        let search_index =
            Arc::new(MemoryIndex::open_or_create(&layout.search_index_dir()).unwrap());
        let reflector = Reflector::new(
            Box::new(crate::memory::test_helpers::MockMemoryProvider::new(
                r#"{"observations": [{"content": "compressed", "timestamp": "2026-02-21T14:30", "visibility": "user"}]}"#,
            )),
            ReflectorConfig::default(),
        );
        let mw = MemoryMergeWriter::new(reflector, layout.clone(), search_index, None, None);

        let compressed = mw.force_reflect().await.unwrap();
        assert_eq!(compressed.observations.len(), 1);
        assert_eq!(
            compressed.observations.first().unwrap().content,
            "compressed"
        );
    }

    #[tokio::test]
    async fn merge_refuses_a_run_id_already_merged() {
        let dir = tempfile::tempdir().unwrap();
        let mw = writer(dir.path());
        let tag = SourceTag::session("spawned-researcher-0001", "run-dup", "spawned");

        let first = mw
            .merge(sample_extraction("first pass"), tag.clone(), chrono_tz::UTC)
            .await
            .unwrap();

        let second = mw
            .merge(sample_extraction("second pass"), tag, chrono_tz::UTC)
            .await
            .unwrap();

        assert_eq!(
            second.id, first.id,
            "a second merge for the same run id must return the existing episode, not mint a new one"
        );

        let layout = WorkspaceLayout::new(dir.path());
        let log = crate::memory::log_store::load_observation_log(&layout.observations_json())
            .await
            .unwrap();
        assert_eq!(
            log.observations.len(),
            1,
            "the duplicate merge must not append a second observation"
        );
        let latest = crate::memory::episode_store::latest_episode_id(&layout.episodes_dir())
            .await
            .unwrap();
        assert_eq!(
            latest,
            Some(first.id),
            "the duplicate merge must not create a second episode"
        );
    }

    #[tokio::test]
    async fn merge_still_allocates_new_ids_for_different_run_ids() {
        let dir = tempfile::tempdir().unwrap();
        let mw = writer(dir.path());

        let a = mw
            .merge(
                sample_extraction("a"),
                SourceTag::session("spawned-a", "run-a", "spawned"),
                chrono_tz::UTC,
            )
            .await
            .unwrap();
        let b = mw
            .merge(
                sample_extraction("b"),
                SourceTag::session("spawned-b", "run-b", "spawned"),
                chrono_tz::UTC,
            )
            .await
            .unwrap();

        assert_ne!(a.id, b.id, "different run ids must get distinct episodes");
    }

    /// An embedding provider that sleeps before returning, standing in for a
    /// slow network call.
    struct SlowEmbeddingProvider {
        delay: std::time::Duration,
    }

    #[async_trait::async_trait]
    impl EmbeddingProvider for SlowEmbeddingProvider {
        async fn embed(
            &self,
            texts: &[&str],
        ) -> Result<crate::inference::EmbeddingResponse, crate::inference::InferenceError> {
            tokio::time::sleep(self.delay).await;
            Ok(crate::inference::EmbeddingResponse {
                embeddings: texts.iter().map(|_| vec![0.0_f32; 4]).collect(),
                dimensions: 4,
            })
        }

        fn model_name(&self) -> &'static str {
            "slow-embed"
        }
    }

    #[tokio::test]
    async fn slow_embedding_does_not_block_a_concurrent_merges_episode_allocation() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        let search_index =
            Arc::new(MemoryIndex::open_or_create(&layout.search_index_dir()).unwrap());
        let vector_store = Arc::new(VectorStore::open_or_create(&layout.vectors_db(), 4).unwrap());
        // A never-triggering reflector isolates this test to the embedding
        // step alone.
        let reflector = Reflector::new(
            Box::new(crate::inference::providers::null::NullProvider),
            ReflectorConfig {
                threshold_tokens: usize::MAX,
                ..ReflectorConfig::default()
            },
        );
        let slow_delay = std::time::Duration::from_millis(400);
        let mw = Arc::new(MemoryMergeWriter::new(
            reflector,
            layout.clone(),
            search_index,
            Some(vector_store),
            Some(Arc::new(SlowEmbeddingProvider { delay: slow_delay })),
        ));

        let mw1 = Arc::clone(&mw);
        let first = tokio::spawn(async move {
            mw1.merge(
                sample_extraction("first"),
                SourceTag::main(),
                chrono_tz::UTC,
            )
            .await
            .unwrap()
        });

        // Give the first merge time to release the merge lock and enter its
        // slow embedding step.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let mw2 = Arc::clone(&mw);
        let second = tokio::spawn(async move {
            mw2.merge(
                sample_extraction("second"),
                SourceTag::main(),
                chrono_tz::UTC,
            )
            .await
            .unwrap()
        });

        // The second merge's own indexing/embedding step also uses the slow
        // provider and legitimately waits its turn for the shared reflector
        // state lock — that contention is expected and not what this test
        // checks. What must not be blocked is episode id allocation and the
        // durable per-episode writes (`persist_episode`), which happen
        // entirely before either merge ever touches that lock. Checking the
        // filesystem directly — well inside the embedding delay, regardless
        // of whether either `merge()` call has returned yet — proves that.
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        let latest = crate::memory::episode_store::latest_episode_id(&layout.episodes_dir())
            .await
            .unwrap();
        assert_eq!(
            latest,
            Some("ep-002".to_string()),
            "the second merge's episode id allocation and durable writes must complete \
             well before the first merge's slow embedding step does"
        );

        let first_outcome = first.await.unwrap();
        let second_outcome = second.await.unwrap();
        assert_eq!(first_outcome.id, "ep-001");
        assert_eq!(second_outcome.id, "ep-002");
    }
}
