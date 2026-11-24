use std::sync::Arc;

use anyhow::Context;
use chrono_tz::Tz;
use tokio::sync::Mutex;

use crate::chunk_extractor::{extract_chunks, write_idx_jsonl};
use crate::episode_store::{
    episode_idx_path, episode_obs_path, next_episode_id, write_completion_marker,
    write_episode_transcript_tagged,
};
use crate::log_store::{append_observations, load_observation_log, save_episode_observations};
use crate::observer::Extraction;
use crate::reflector::{Reflector, ReflectorConfig, replace_with_compressed};
use crate::search::MemoryIndex;
use crate::types::{
    Episode, IndexChunk, IndexManifest, ManifestFileEntry, Observation, ObservationLog, SourceTag,
};
use crate::vector_store::VectorStore;
use residuum_config::WorkspaceLayout;
use residuum_core::time::now_local;
use residuum_inference::EmbeddingProvider;

pub struct MergeOutcome {
    pub id: String,
    pub transcript_path: std::path::PathBuf,
    pub narrative: Option<String>,
    pub observations: Vec<Observation>,
    pub chunks: Vec<IndexChunk>,
    pub date: String,
    pub reflected: bool,
    pub reflector_notice: Option<residuum_core::util::NoticeAction>,
}

struct MergeState {
    reflector: Reflector,
    embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
}

pub struct MemoryMergeWriter {
    merge_lock: Mutex<()>,
    log_lock: Mutex<()>,
    reflection_lock: Mutex<()>,
    state: Mutex<MergeState>,
    layout: WorkspaceLayout,
    search_index: Arc<MemoryIndex>,
    vector_store: Option<Arc<VectorStore>>,
    reflector_failure: residuum_core::util::BackoffTracker,
}

impl MemoryMergeWriter {
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
            reflection_lock: Mutex::new(()),
            state: Mutex::new(MergeState {
                reflector,
                embedding_provider,
            }),
            layout,
            search_index,
            vector_store,
            reflector_failure: residuum_core::util::BackoffTracker::new(),
        }
    }

    #[must_use]
    pub fn reflector_failure_tracker(&self) -> &residuum_core::util::BackoffTracker {
        &self.reflector_failure
    }

    pub async fn update_reflector_config(&self, config: ReflectorConfig) {
        self.state.lock().await.reflector.update_config(config);
    }

    pub async fn swap_reflector(&self, reflector: Reflector) {
        self.state.lock().await.reflector = reflector;
        self.reflector_failure.clear_backoff_wait();
    }

    pub async fn set_embedding_provider(
        &self,
        embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
    ) {
        self.state.lock().await.embedding_provider = embedding_provider;
    }

    async fn snapshot_state(&self) -> (Reflector, Option<Arc<dyn EmbeddingProvider>>) {
        let state = self.state.lock().await;
        (state.reflector.clone(), state.embedding_provider.clone())
    }

    pub async fn force_reflect(&self) -> anyhow::Result<ObservationLog> {
        let _reflection = self.reflection_lock.lock().await;
        let (reflector, _) = self.snapshot_state().await;
        let log = load_observation_log(&self.layout.observations_json()).await?;
        self.reflect_and_replace(&reflector, log).await
    }

    async fn reflect_and_replace(
        &self,
        reflector: &Reflector,
        log: ObservationLog,
    ) -> anyhow::Result<ObservationLog> {
        if log.observations.is_empty() {
            return Ok(log);
        }
        let compressed = reflector.compress(&self.layout, &log).await?;
        let _log_guard = self.log_lock.lock().await;
        replace_with_compressed(&self.layout, &log, compressed).await
    }

    async fn check_already_merged(&self, tag: &SourceTag) -> anyhow::Result<Option<MergeOutcome>> {
        let Some(run_id) = tag.run_id.as_deref() else {
            return Ok(None);
        };
        let merged =
            crate::merged_run_log::load_merged_runs(&self.layout.merged_runs_json()).await?;
        let Some(episode_id) = merged.get(run_id).cloned() else {
            return Ok(None);
        };
        tracing::info!(run_id, episode_id = %episode_id, "run already merged into global memory; refusing duplicate merge");
        let transcript_path =
            crate::episode_store::find_episode_path(&self.layout.episodes_dir(), &episode_id)?
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
            reflector_notice: None,
        }))
    }

    pub async fn find_merged_episode(&self, run_id: &str) -> anyhow::Result<Option<String>> {
        Ok(
            crate::merged_run_log::load_merged_runs(&self.layout.merged_runs_json())
                .await?
                .get(run_id)
                .cloned(),
        )
    }

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
        let (reflector, embedding_provider) = self.snapshot_state().await;
        let (reflected, reflector_notice) = self
            .index_embed_and_reflect(
                &reflector,
                embedding_provider.as_deref(),
                &episode.id,
                &date_str,
                &observations,
                &chunks,
            )
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
            reflector_notice: match reflector_notice {
                residuum_core::util::NoticeAction::None => None,
                started @ residuum_core::util::NoticeAction::FailureStarted => Some(started),
                recovered @ residuum_core::util::NoticeAction::Recovered => Some(recovered),
            },
        })
    }

    async fn persist_episode(
        &self,
        episode: &Episode,
        extraction: &Extraction,
        tag: &SourceTag,
    ) -> anyhow::Result<(std::path::PathBuf, Vec<Observation>, Vec<IndexChunk>)> {
        let transcript_path =
            crate::episode_store::episode_jsonl_path(&self.layout.episodes_dir(), episode);
        let messages: Vec<residuum_inference::Message> = extraction
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
            crate::merged_run_log::record_merged_run(
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

    async fn index_embed_and_reflect(
        &self,
        reflector: &Reflector,
        embedding_provider: Option<&dyn EmbeddingProvider>,
        episode_id: &str,
        date_str: &str,
        observations: &[Observation],
        chunks: &[IndexChunk],
    ) -> (bool, residuum_core::util::NoticeAction) {
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
            embedding_provider,
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

        let Ok(_reflection) = self.reflection_lock.try_lock() else {
            tracing::debug!("a reflection is already running, skipping this check");
            return (false, residuum_core::util::NoticeAction::None);
        };
        let log = load_observation_log(&self.layout.observations_json())
            .await
            .unwrap_or_default();
        if !reflector.should_reflect(&log) {
            return (false, residuum_core::util::NoticeAction::None);
        }
        if self.reflector_failure.gate() == residuum_core::util::RetryGate::Skip {
            tracing::debug!(
                "reflector is backing off after a recent failure, skipping this attempt"
            );
            return (false, residuum_core::util::NoticeAction::None);
        }
        match self.reflect_and_replace(reflector, log).await {
            Ok(compressed) => {
                tracing::info!(
                    episodes = compressed.observations.len(),
                    "reflector compressed observation log"
                );
                (true, self.reflector_failure.record_success())
            }
            Err(e) => {
                tracing::warn!(error = %e, "reflector failed");
                (false, self.reflector_failure.record_failure())
            }
        }
    }
}

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

fn episode_date_dir(date: &str) -> Option<String> {
    let year_month = date.get(..7)?;
    let day = date.get(8..10)?;
    Some(format!("{year_month}/{day}"))
}

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
    use crate::observer::ExtractedObservation;
    use crate::recent_messages::RecentMessage;
    use crate::reflector::ReflectorConfig;
    use crate::types::Visibility;
    use residuum_inference::Message;
    use std::path::Path;

    fn writer(dir: &Path) -> MemoryMergeWriter {
        let layout = WorkspaceLayout::new(dir);
        let search_index =
            Arc::new(MemoryIndex::open_or_create(&layout.search_index_dir()).unwrap());
        let reflector = Reflector::new(
            Box::new(residuum_inference::NullProvider),
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
                turn_id: None,
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
        let log = crate::log_store::load_observation_log(&layout.observations_json())
            .await
            .unwrap();
        assert_eq!(log.observations.len(), 1);
    }

    fn writer_with_always_failing_reflector(dir: &Path) -> MemoryMergeWriter {
        let layout = WorkspaceLayout::new(dir);
        let search_index =
            Arc::new(MemoryIndex::open_or_create(&layout.search_index_dir()).unwrap());
        let reflector = Reflector::new(
            Box::new(residuum_inference::NullProvider),
            ReflectorConfig {
                threshold_tokens: 0,
                ..ReflectorConfig::default()
            },
        );
        MemoryMergeWriter::new(reflector, layout, search_index, None, None)
    }

    #[tokio::test]
    async fn reflector_failure_notifies_once_then_backs_off() {
        let dir = tempfile::tempdir().unwrap();
        let mw = writer_with_always_failing_reflector(dir.path());

        let first = mw
            .merge(sample_extraction("one"), SourceTag::main(), chrono_tz::UTC)
            .await
            .unwrap();
        assert!(!first.reflected);
        assert_eq!(
            first.reflector_notice,
            Some(residuum_core::util::NoticeAction::FailureStarted)
        );

        let second = mw
            .merge(sample_extraction("two"), SourceTag::main(), chrono_tz::UTC)
            .await
            .unwrap();
        assert!(!second.reflected);
        assert_eq!(
            second.reflector_notice, None,
            "must not re-attempt (and re-spend) while backing off, nor renotify"
        );

        assert!(mw.force_reflect().await.is_err());
        assert_eq!(
            mw.reflector_failure_tracker().gate(),
            residuum_core::util::RetryGate::Skip,
            "the failed manual retry should extend, not clear, the automatic backoff"
        );
    }

    fn writer_with_gated_reflector(
        dir: &Path,
        gate: &residuum_core::testing::gate::Gate,
    ) -> MemoryMergeWriter {
        let layout = WorkspaceLayout::new(dir);
        let search_index =
            Arc::new(MemoryIndex::open_or_create(&layout.search_index_dir()).unwrap());
        let reflector = Reflector::new(
            Box::new(residuum_inference::testing::model::GatedProvider::new(
                gate,
                r#"{"observations": [{"content": "compressed", "timestamp": "2026-02-21T14:30", "visibility": "user"}]}"#,
            )),
            ReflectorConfig {
                threshold_tokens: 0,
                ..ReflectorConfig::default()
            },
        );
        MemoryMergeWriter::new(reflector, layout, search_index, None, None)
    }

    #[tokio::test]
    async fn a_running_reflection_holds_up_neither_merges_nor_reloads() {
        use residuum_core::testing::wait::guarded;

        let dir = tempfile::tempdir().unwrap();
        let gate = residuum_core::testing::gate::Gate::closed("reflector");
        let mw = Arc::new(writer_with_gated_reflector(dir.path(), &gate));

        let reflecting = tokio::spawn({
            let mw = Arc::clone(&mw);
            async move {
                mw.merge(sample_extraction("one"), SourceTag::main(), chrono_tz::UTC)
                    .await
            }
        });
        gate.until_held(1).await;

        let second = guarded(
            "a merge while the reflector's model call is held",
            mw.merge(sample_extraction("two"), SourceTag::main(), chrono_tz::UTC),
        )
        .await
        .unwrap();
        assert!(
            !second.reflected,
            "a merge that finds a reflection running should skip its own"
        );
        guarded(
            "a reflector config reload while the reflector's model call is held",
            mw.update_reflector_config(ReflectorConfig::default()),
        )
        .await;

        gate.open_all();
        let first = guarded("the held reflection to finish", reflecting)
            .await
            .unwrap()
            .unwrap();
        assert!(first.reflected, "the first merge's reflection should land");

        let layout = WorkspaceLayout::new(dir.path());
        let log = crate::log_store::load_observation_log(&layout.observations_json())
            .await
            .unwrap();
        let contents: Vec<&str> = log
            .observations
            .iter()
            .map(|o| o.content.as_str())
            .collect();
        assert_eq!(
            contents,
            ["compressed", "two"],
            "the merge made during the reflection should survive it"
        );
    }

    #[tokio::test]
    async fn force_reflect_on_an_empty_log_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mw = writer_with_always_failing_reflector(dir.path());

        let log = mw.force_reflect().await.unwrap();
        assert!(log.observations.is_empty(), "empty log should stay empty");
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

        let (meta, _) = crate::episode_store::read_episode_jsonl(&outcome.transcript_path)
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
            handles.push(residuum_core::util::spawn_in_span(async move {
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
        let log = crate::log_store::load_observation_log(&layout.observations_json())
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
        let mut log = crate::types::ObservationLog::new();
        log.observations.push(crate::types::Observation {
            timestamp: chrono::Utc::now().naive_utc(),
            source_episodes: Some("ep-001".to_string()),
            visibility: Visibility::User,
            content: "an observation".to_string(),
            source: SourceTag::main(),
        });
        crate::log_store::save_observation_log(&layout.observations_json(), &log)
            .await
            .unwrap();

        let search_index =
            Arc::new(MemoryIndex::open_or_create(&layout.search_index_dir()).unwrap());
        let reflector = Reflector::new(
            Box::new(crate::testing::MockMemoryProvider::new(
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
        let log = crate::log_store::load_observation_log(&layout.observations_json())
            .await
            .unwrap();
        assert_eq!(
            log.observations.len(),
            1,
            "the duplicate merge must not append a second observation"
        );
        let latest = crate::episode_store::latest_episode_id(&layout.episodes_dir()).unwrap();
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

    struct GatedEmbeddingProvider {
        entered: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Semaphore>,
    }

    #[async_trait::async_trait]
    impl EmbeddingProvider for GatedEmbeddingProvider {
        async fn embed(
            &self,
            texts: &[&str],
        ) -> Result<residuum_inference::EmbeddingResponse, residuum_inference::InferenceError>
        {
            self.entered.notify_one();
            self.release.acquire().await.unwrap().forget();
            Ok(residuum_inference::EmbeddingResponse {
                embeddings: texts.iter().map(|_| vec![0.0_f32; 4]).collect(),
                dimensions: 4,
            })
        }

        fn model_name(&self) -> &'static str {
            "gated-embed"
        }
    }

    #[tokio::test]
    async fn slow_embedding_does_not_block_a_concurrent_merges_episode_allocation() {
        let dir = tempfile::tempdir().unwrap();
        let layout = WorkspaceLayout::new(dir.path());
        let search_index =
            Arc::new(MemoryIndex::open_or_create(&layout.search_index_dir()).unwrap());
        let vector_store = Arc::new(VectorStore::open_or_create(&layout.vectors_db(), 4).unwrap());
        let reflector = Reflector::new(
            Box::new(residuum_inference::NullProvider),
            ReflectorConfig {
                threshold_tokens: usize::MAX,
                ..ReflectorConfig::default()
            },
        );
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let mw = Arc::new(MemoryMergeWriter::new(
            reflector,
            layout.clone(),
            search_index,
            Some(vector_store),
            Some(Arc::new(GatedEmbeddingProvider {
                entered: Arc::clone(&entered),
                release: Arc::clone(&release),
            })),
        ));

        let mw1 = Arc::clone(&mw);
        let first = residuum_core::util::spawn_in_span(async move {
            mw1.merge(
                sample_extraction("first"),
                SourceTag::main(),
                chrono_tz::UTC,
            )
            .await
            .unwrap()
        });

        entered.notified().await;

        let mw2 = Arc::clone(&mw);
        let second = residuum_core::util::spawn_in_span(async move {
            mw2.merge(
                sample_extraction("second"),
                SourceTag::main(),
                chrono_tz::UTC,
            )
            .await
            .unwrap()
        });

        residuum_core::testing::wait::until(
            "the second merge's episode id allocation and durable writes to complete \
             while the first merge is still inside its embedding step",
            || async {
                let latest =
                    crate::episode_store::latest_episode_id(&layout.episodes_dir()).unwrap();
                (latest.as_deref() == Some("ep-002")).then_some(())
            },
        )
        .await;

        release.add_permits(2);
        let first_outcome = first.await.unwrap();
        let second_outcome = second.await.unwrap();
        assert_eq!(first_outcome.id, "ep-001");
        assert_eq!(second_outcome.id, "ep-002");
    }
}
