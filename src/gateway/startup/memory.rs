//! Memory system initialization: search index, vector store, and embeddings pipeline.

use std::path::Path;
use std::sync::Arc;

use crate::config::Config;
use crate::error::FatalError;
use crate::memory::chunk_extractor::read_idx_jsonl;
use crate::memory::observer::{Observer, ObserverConfig};
use crate::memory::reflector::{Reflector, ReflectorConfig};
use crate::memory::search::{HybridSearcher, MemoryIndex, RebuildResult, parse_obs_file};
use crate::memory::types::IndexManifest;
use crate::memory::vector_store::VectorStore;
use crate::models::{EmbeddingProvider, SharedHttpClient, build_provider_chain};
use crate::workspace::layout::WorkspaceLayout;
use anyhow::Context;

/// Search index and vector store built during initialization.
pub(super) struct MemoryComponents {
    pub search_index: Arc<MemoryIndex>,
    pub hybrid_searcher: Arc<HybridSearcher>,
    pub vector_store: Option<Arc<VectorStore>>,
}

/// Build observer and reflector from fully-resolved provider specs on `Config`.
///
/// # Errors
/// Returns `FatalError::Config` if either provider cannot be built.
pub(super) fn build_memory_components(
    cfg: &Config,
    tz: chrono_tz::Tz,
    http: SharedHttpClient,
) -> Result<(Observer, Reflector), FatalError> {
    let observer_provider = build_provider_chain(
        &cfg.observer,
        cfg.max_tokens,
        http.clone(),
        cfg.retry.clone(),
    )?;
    let reflector_provider =
        build_provider_chain(&cfg.reflector, cfg.max_tokens, http, cfg.retry.clone())?;

    let observer = Observer::new(
        observer_provider,
        ObserverConfig {
            tz,
            role_overrides: cfg.role_overrides.get("observer").cloned(),
            ..ObserverConfig::default()
        },
    );

    let reflector = Reflector::new(
        reflector_provider,
        ReflectorConfig {
            tz,
            role_overrides: cfg.role_overrides.get("reflector").cloned(),
            ..ReflectorConfig::default()
        },
    );

    Ok((observer, reflector))
}

/// Build the search index, vector store, and hybrid searcher.
///
/// # Errors
/// Returns `FatalError` if the search index cannot be created.
pub(super) async fn init_memory(
    cfg: &Config,
    layout: &WorkspaceLayout,
    embedding_provider: Option<&Arc<dyn EmbeddingProvider>>,
) -> Result<MemoryComponents, FatalError> {
    // Search index — schema migration + incremental sync
    let manifest_path = layout.index_manifest_json();
    let manifest = match IndexManifest::load(&manifest_path).await {
        Ok(m) => m,
        Err(err) => {
            tracing::warn!(error = %err, "index manifest degraded: starting with empty manifest");
            IndexManifest::default()
        }
    };

    // If no manifest exists but old index dir does, clear it (schema migration)
    if manifest.files.is_empty()
        && layout.search_index_dir().exists()
        && let Err(migration_err) = std::fs::remove_dir_all(layout.search_index_dir())
    {
        tracing::warn!(error = %migration_err, "failed to clear old search index for schema migration");
    }

    let search_index = match MemoryIndex::open_or_create(&layout.search_index_dir()) {
        Ok(idx) => Arc::new(idx),
        Err(err) => {
            tracing::warn!(error = %err, "search index degraded: using empty in-memory index");
            Arc::new(MemoryIndex::empty()?)
        }
    };

    sync_search_index(&search_index, &manifest, layout, &manifest_path).await;

    // Vector store (only if embedding provider is configured)
    let vector_store: Option<Arc<VectorStore>> = if let Some(ep) = embedding_provider {
        build_vector_store(ep.as_ref(), layout, &manifest, &manifest_path).await
    } else {
        None
    };

    // Backfill embeddings for any unembedded files
    if let (Some(vs), Some(ep)) = (&vector_store, embedding_provider) {
        backfill_embeddings(vs, ep.as_ref(), layout, &manifest_path).await;
    }

    // Hybrid searcher
    let hybrid_searcher = Arc::new(HybridSearcher::new(
        Arc::clone(&search_index),
        vector_store.clone(),
        embedding_provider.cloned(),
        cfg.memory.search.clone(),
    ));

    Ok(MemoryComponents {
        search_index,
        hybrid_searcher,
        vector_store,
    })
}

/// Perform a full search index rebuild and save the resulting manifest.
async fn do_full_rebuild(
    search_index: &MemoryIndex,
    layout: &WorkspaceLayout,
    manifest_path: &Path,
) {
    match search_index.rebuild(&layout.memory_dir()) {
        Ok(result) => {
            let total = result.obs_count + result.chunk_count;
            tracing::info!(
                observations = result.obs_count,
                chunks = result.chunk_count,
                "search index rebuilt ({total} documents)"
            );
            let rebuilt = build_manifest_from_rebuild(result);
            if let Err(save_err) = rebuilt.save(manifest_path).await {
                tracing::warn!(error = %save_err, "failed to save index manifest after rebuild");
            }
        }
        Err(rebuild_err) => {
            tracing::warn!(error = %rebuild_err, "failed to rebuild search index");
        }
    }
}

/// Synchronize the search index (full rebuild or incremental sync).
async fn sync_search_index(
    search_index: &MemoryIndex,
    manifest: &IndexManifest,
    layout: &WorkspaceLayout,
    manifest_path: &Path,
) {
    if manifest.files.is_empty() {
        do_full_rebuild(search_index, layout, manifest_path).await;
    } else {
        match search_index.incremental_sync(&layout.memory_dir(), manifest) {
            Ok((synced_manifest, stats)) => {
                tracing::info!(
                    added = stats.added,
                    updated = stats.updated,
                    removed = stats.removed,
                    unchanged = stats.unchanged,
                    "search index synced incrementally"
                );
                if let Err(save_err) = synced_manifest.save(manifest_path).await {
                    tracing::warn!(error = %save_err, "failed to save index manifest after sync");
                }
            }
            Err(sync_err) => {
                tracing::warn!(error = %sync_err, "incremental sync failed, falling back to full rebuild");
                do_full_rebuild(search_index, layout, manifest_path).await;
            }
        }
    }
}

/// Build the vector store, probing for embedding dimensions.
///
/// `manifest` is used only to check whether the embedding model has changed.
/// The function re-reads the manifest from `manifest_path` before saving, because
/// `sync_search_index` may have written an updated manifest to disk since the
/// caller loaded it.
async fn build_vector_store(
    ep: &dyn EmbeddingProvider,
    layout: &WorkspaceLayout,
    manifest: &IndexManifest,
    manifest_path: &Path,
) -> Option<Arc<VectorStore>> {
    let probe = match ep.embed(&["dimension probe"]).await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "embedding dimension probe failed");
            return None;
        }
    };

    let dim = probe.dimensions;
    let model_name = ep.model_name().to_string();

    let model_changed = manifest
        .embedding_model
        .as_ref()
        .is_some_and(|m| *m != model_name);
    if model_changed {
        tracing::info!(
            old_model = manifest.embedding_model.as_deref().unwrap_or("none"),
            new_model = model_name.as_str(),
            "embedding model changed, clearing vector store"
        );
        if let Err(e) = std::fs::remove_file(layout.vectors_db())
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(error = %e, "failed to remove old vector store");
        }
    }

    match VectorStore::open_or_create(&layout.vectors_db(), dim) {
        Ok(vs) => {
            tracing::info!(dim, model = model_name.as_str(), "vector store ready");

            let mut updated_manifest = IndexManifest::load(manifest_path).await.unwrap_or_default();
            updated_manifest.embedding_model = Some(model_name);
            updated_manifest.embedding_dim = Some(dim);
            if model_changed {
                for entry in updated_manifest.files.values_mut() {
                    entry.embedded = false;
                }
            }
            if let Err(e) = updated_manifest.save(manifest_path).await {
                tracing::warn!(error = %e, "failed to save manifest with embedding info");
            }

            Some(Arc::new(vs))
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to open vector store");
            None
        }
    }
}

/// Build an `IndexManifest` from a full rebuild result.
fn build_manifest_from_rebuild(result: RebuildResult) -> IndexManifest {
    let mut manifest = IndexManifest::new();
    manifest.last_rebuild = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S").to_string();
    for (path, entry) in result.file_entries {
        manifest.files.insert(path, entry);
    }
    manifest
}

/// Embed any manifest entries that have `embedded: false` into the vector store.
///
/// Reads each unembedded `.obs.json` or `.idx.jsonl` file from disk, calls the
/// embedding provider, and inserts into the vector store. Failures are warnings
/// and never block startup.
async fn backfill_embeddings(
    vs: &VectorStore,
    ep: &dyn EmbeddingProvider,
    layout: &WorkspaceLayout,
    manifest_path: &Path,
) {
    let mut manifest = match IndexManifest::load(manifest_path).await {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(error = %e, "failed to load manifest for embedding backfill");
            return;
        }
    };

    let unembedded: Vec<String> = manifest
        .files
        .iter()
        .filter(|(_, entry)| !entry.embedded)
        .map(|(path, _)| path.clone())
        .collect();

    if unembedded.is_empty() {
        return;
    }

    tracing::info!(
        count = unembedded.len(),
        "backfilling embeddings for unembedded files"
    );
    let memory_dir = layout.memory_dir();
    let mut embedded_count = 0_usize;

    for rel_path in &unembedded {
        let abs_path = memory_dir.join(rel_path);

        if rel_path.ends_with(".obs.json") {
            if let Err(e) = backfill_obs_file(vs, ep, &abs_path).await {
                tracing::warn!(error = %e, path = %rel_path, "failed to backfill embeddings");
                continue;
            }
        } else if rel_path.ends_with(".idx.jsonl") {
            if let Err(e) = backfill_idx_file(vs, ep, &abs_path).await {
                tracing::warn!(error = %e, path = %rel_path, "failed to backfill embeddings");
                continue;
            }
        } else {
            continue;
        }

        if let Some(entry) = manifest.files.get_mut(rel_path) {
            entry.embedded = true;
        }
        embedded_count += 1;
    }

    if embedded_count > 0 {
        if let Err(e) = manifest.save(manifest_path).await {
            tracing::warn!(error = %e, "failed to save manifest after embedding backfill");
        }
        tracing::info!(embedded_count, "embedding backfill complete");
    }
}

/// Embed a single `.obs.json` file and insert into the vector store.
///
/// Skips the embedding API call if vectors already exist in the store.
async fn backfill_obs_file(
    vs: &VectorStore,
    ep: &dyn EmbeddingProvider,
    path: &Path,
) -> anyhow::Result<()> {
    let (episode_id, date, observations) = parse_obs_file(path)?;
    if observations.is_empty() {
        return Ok(());
    }

    // Check if vectors already exist (e.g. embedded inline but manifest wasn't updated)
    let first_id = format!("{episode_id}-o0");
    if vs.has_observation(&first_id)? {
        tracing::debug!(episode_id, "skipping obs backfill — vectors already exist");
        return Ok(());
    }

    let texts: Vec<&str> = observations.iter().map(|o| o.content.as_str()).collect();
    let response = ep
        .embed(&texts)
        .await
        .with_context(|| format!("embedding failed for {}", path.display()))?;

    let embeddings = response.embeddings;
    vs.insert_observations(&episode_id, &date, &observations, &embeddings)?;
    Ok(())
}

/// Embed a single `.idx.jsonl` file and insert into the vector store.
///
/// Skips the embedding API call if vectors already exist in the store.
async fn backfill_idx_file(
    vs: &VectorStore,
    ep: &dyn EmbeddingProvider,
    path: &Path,
) -> anyhow::Result<()> {
    let chunks = read_idx_jsonl(path);
    if chunks.is_empty() {
        return Ok(());
    }

    // Check if vectors already exist (e.g. embedded inline but manifest wasn't updated)
    if let Some(first) = chunks.first()
        && vs.has_chunk(&first.chunk_id)?
    {
        tracing::debug!(
            chunk_id = first.chunk_id,
            "skipping idx backfill — vectors already exist"
        );
        return Ok(());
    }

    let texts: Vec<&str> = chunks.iter().map(|c| c.content.as_str()).collect();
    let response = ep
        .embed(&texts)
        .await
        .with_context(|| format!("embedding failed for {}", path.display()))?;

    let embeddings = response.embeddings;
    vs.insert_chunks(&chunks, &embeddings)?;
    Ok(())
}
