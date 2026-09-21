//! Memory merge writer: the single serialized writer for global memory.
//!
//! Episode id allocation, the observation log append, the episode transcript
//! and its search-chunk index, embedding, and the reflector trigger all go
//! through [`MemoryMergeWriter`]. The main agent's own observation flow and
//! every session run's completion pipeline call the same writer, so episode
//! numbering and log appends never race between concurrent runs.

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

/// State that must be mutated under the single merge lock: the reflector
/// (updated on config reload) and the embedding provider (swapped on
/// provider reload). Kept together so a reload's writes and a concurrent
/// merge's reads never interleave.
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
        state.reflector.reflect(&self.layout).await
    }

    /// Merge an extraction into global memory: allocate an episode id, write
    /// the transcript and observation archives, index and embed, and check
    /// whether the reflector should run.
    ///
    /// Serialized against every other merge (main agent or session) through
    /// the writer's internal lock, so episode numbering and log appends
    /// never race.
    ///
    /// # Errors
    /// Returns an error if the episode id cannot be allocated or the
    /// transcript cannot be written. Indexing, embedding, and reflector
    /// failures are logged as warnings and never fail the merge — the
    /// episode transcript and observation log are the durable record.
    #[tracing::instrument(skip_all, fields(session_address = tag.session_address.as_deref().unwrap_or("main")))]
    pub async fn merge(
        &self,
        extraction: Extraction,
        tag: SourceTag,
        tz: Tz,
    ) -> anyhow::Result<MergeOutcome> {
        let state = self.state.lock().await;

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
        let date_str = episode.date.to_string();
        let reflected = self
            .index_embed_and_reflect(&state, &episode.id, &date_str, &observations, &chunks)
            .await;

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
    /// global observation archives, the interaction-pair chunk index, and
    /// the completion marker. Returns the transcript path, the tagged
    /// observations, and the extracted chunks for the caller to index.
    ///
    /// # Errors
    /// Returns an error if the transcript or the global observation log
    /// cannot be written — these are the durable record a merge must not
    /// silently lose. Per-episode archive and chunk-index write failures are
    /// logged as warnings only; a missing archive just means startup sync
    /// re-indexes that episode later.
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
        if let Err(e) = save_episode_observations(&obs_path, &observations).await {
            tracing::warn!(episode_id = %episode.id, error = %e, "failed to write per-episode observation archive");
        }
        append_observations(&self.layout.observations_json(), observations.clone())
            .await
            .context("failed to append to global observation log")?;
        tracing::debug!(episode_id = %episode.id, count = observations.len(), "global observations updated");

        let date_str = episode.date.to_string();
        let chunks = extract_chunks(&extraction.messages, &episode.id, &date_str, 2);
        let idx_path = episode_idx_path(&self.layout.episodes_dir(), episode);
        if let Err(e) = write_idx_jsonl(&idx_path, &chunks).await {
            tracing::warn!(episode_id = %episode.id, error = %e, "failed to write interaction-pair chunk index");
        }

        if let Err(e) = write_completion_marker(&self.layout.episodes_dir(), episode).await {
            tracing::warn!(episode_id = %episode.id, error = %e, "failed to write episode completion marker");
        }

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
}
