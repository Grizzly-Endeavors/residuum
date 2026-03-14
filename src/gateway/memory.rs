//! Memory pipeline helpers: observation, reflection, and persistence.

use std::sync::Arc;

use crate::agent::Agent;
use crate::bus::{BusEvent, Publisher, TopicId};
use crate::memory::log_store::load_observation_log;
use crate::memory::observer::{ObserveAction, ObserveResult, Observer};
use crate::memory::recent_context::{RecentContext, save_recent_context};
use crate::memory::recent_messages::{
    append_recent_messages, clear_recent_messages, load_recent_messages,
};
use crate::memory::reflector::Reflector;
use crate::memory::search::MemoryIndex;
use crate::memory::types::{IndexManifest, ManifestFileEntry, Visibility};
use crate::memory::vector_store::VectorStore;
use crate::models::EmbeddingProvider;
use crate::workspace::layout::WorkspaceLayout;

/// Date format for episode file paths: `YYYY-MM/DD`.
fn episode_date_dir(date: &str) -> Option<String> {
    // date is "YYYY-MM-DD" → "YYYY-MM/DD"
    let year_month = date.get(..7)?;
    let day = date.get(8..10)?;
    Some(format!("{year_month}/{day}"))
}

/// Persist new messages and check whether observation thresholds are met.
///
/// Appends messages to the recent messages file and returns the appropriate
/// `ObserveAction` based on current token levels.
pub(super) async fn persist_and_check_thresholds(
    new_messages: &[crate::models::Message],
    project_context: &str,
    visibility: Visibility,
    observer: &Observer,
    layout: &WorkspaceLayout,
    tz: chrono_tz::Tz,
) -> ObserveAction {
    if new_messages.is_empty() {
        return ObserveAction::None;
    }

    if let Err(e) = append_recent_messages(
        &layout.recent_messages_json(),
        new_messages,
        project_context,
        visibility,
        tz,
    )
    .await
    {
        tracing::warn!(error = %e, "failed to persist recent messages");
        return ObserveAction::None;
    }

    let recent = match load_recent_messages(&layout.recent_messages_json()).await {
        Ok(msgs) => msgs,
        Err(e) => {
            tracing::warn!(error = %e, "failed to load recent messages");
            return ObserveAction::None;
        }
    };

    observer.check_thresholds(&recent)
}

/// Subsystem references for memory observation and embedding.
pub(super) struct MemorySubsystems<'a> {
    pub observer: &'a Observer,
    pub reflector: &'a Reflector,
    pub search_index: &'a Arc<MemoryIndex>,
    pub layout: &'a WorkspaceLayout,
    pub vector_store: Option<&'a Arc<VectorStore>>,
    pub embedding_provider: Option<&'a Arc<dyn EmbeddingProvider>>,
}

/// Execute an observation cycle: LLM call, clear file, rotate messages, index, reflect, reload.
pub(super) async fn execute_observation(mem: &MemorySubsystems<'_>, agent: &mut Agent) {
    let recent = match load_recent_messages(&mem.layout.recent_messages_json()).await {
        Ok(msgs) => msgs,
        Err(e) => {
            tracing::warn!(error = %e, "failed to load recent messages for observation");
            return;
        }
    };

    if recent.is_empty() {
        return;
    }

    match mem.observer.observe(&recent, mem.layout).await {
        Ok(result) => {
            tracing::info!(episode_id = %result.id, "observer extracted episode");

            // Save narrative context if present
            if let Some(narrative) = &result.narrative {
                let ctx = RecentContext {
                    narrative: narrative.clone(),
                    created_at: crate::time::now_local(mem.observer.timezone()),
                    episode_id: result.id.clone(),
                };
                if let Err(e) = save_recent_context(&mem.layout.recent_context_json(), &ctx).await {
                    tracing::warn!(error = %e, "failed to save recent context");
                }
            }

            if let Err(e) = clear_recent_messages(&mem.layout.recent_messages_json()).await {
                tracing::warn!(error = %e, "failed to clear recent messages");
            }
            agent.rotate_messages_after_observation();

            finalize_observation(mem, agent, &result).await;
        }
        Err(e) => {
            tracing::warn!(error = %e, "observer failed");
        }
    }
}

/// Embed observations and chunks from an observer result into the vector store.
///
/// Silently returns `false` if no embedding provider or vector store is configured.
/// Embedding failures are reported as warnings, never fatal.
/// Returns `true` if all embedding inserts succeeded.
async fn embed_observation_result(
    result: &ObserveResult,
    vector_store: Option<&Arc<VectorStore>>,
    embedding_provider: Option<&Arc<dyn EmbeddingProvider>>,
) -> bool {
    let (Some(vs), Some(ep)) = (vector_store, embedding_provider) else {
        return false;
    };

    let mut all_ok = true;

    // Embed observations
    if !result.observations.is_empty() {
        let texts: Vec<&str> = result
            .observations
            .iter()
            .map(|o| o.content.as_str())
            .collect();
        match ep.embed(&texts).await {
            Ok(response) => {
                let vs = Arc::clone(vs);
                let episode_id = result.id.clone();
                let date = result.date.clone();
                let observations = result.observations.clone();
                let embeddings = response.embeddings;
                match tokio::task::spawn_blocking(move || {
                    vs.insert_observations(&episode_id, &date, &observations, &embeddings)
                })
                .await
                {
                    Ok(Err(e)) => {
                        tracing::warn!(error = %e, "failed to insert observation vectors");
                        all_ok = false;
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "observation vector insert task panicked");
                        all_ok = false;
                    }
                    Ok(Ok(_)) => {}
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "failed to embed observations");
                all_ok = false;
            }
        }
    }

    // Embed chunks
    if !result.chunks.is_empty() {
        let texts: Vec<&str> = result.chunks.iter().map(|c| c.content.as_str()).collect();
        match ep.embed(&texts).await {
            Ok(response) => {
                let vs = Arc::clone(vs);
                let chunks = result.chunks.clone();
                let embeddings = response.embeddings;
                match tokio::task::spawn_blocking(move || vs.insert_chunks(&chunks, &embeddings))
                    .await
                {
                    Ok(Err(e)) => {
                        tracing::warn!(error = %e, "failed to insert chunk vectors");
                        all_ok = false;
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "chunk vector insert task panicked");
                        all_ok = false;
                    }
                    Ok(Ok(_)) => {}
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

/// Post-observation steps: index, embed, reflect check, reload agent context.
///
/// Returns `true` if the reflector was triggered.
async fn finalize_observation(
    mem: &MemorySubsystems<'_>,
    agent: &mut Agent,
    result: &ObserveResult,
) -> bool {
    // Index observations and chunks into the search index
    if let Err(e) =
        mem.search_index
            .index_observations(&result.id, &result.date, &result.observations)
    {
        tracing::warn!(error = %e, "failed to index observations");
    }
    if let Err(e) = mem.search_index.index_chunks(&result.chunks) {
        tracing::warn!(error = %e, "failed to index chunks");
    }

    // Embed and store in vector index
    let embedded = embed_observation_result(result, mem.vector_store, mem.embedding_provider).await;
    if embedded {
        mark_episode_embedded(mem.layout, result).await;
    }

    let reflected = run_reflector_check(mem.reflector, mem.layout).await;

    if let Err(e) = agent.reload_observations(mem.layout).await {
        tracing::warn!(error = %e, "failed to reload observations");
    }
    if let Err(e) = agent.reload_recent_context(mem.layout).await {
        tracing::warn!(error = %e, "failed to reload recent context");
    }

    reflected
}

/// Run the reflector if the observation log exceeds the threshold, returning whether it fired.
async fn run_reflector_check(reflector: &Reflector, layout: &WorkspaceLayout) -> bool {
    let log = match load_observation_log(&layout.observations_json()).await {
        Ok(log) => log,
        Err(e) => {
            tracing::warn!(error = %e, "failed to load observation log for reflection check");
            return false;
        }
    };

    if reflector.should_reflect(&log) {
        match reflector.reflect(layout).await {
            Ok(compressed) => {
                tracing::info!(
                    episodes = compressed.len(),
                    "reflector compressed observation log"
                );
                true
            }
            Err(e) => {
                tracing::warn!(error = %e, "reflector failed");
                false
            }
        }
    } else {
        false
    }
}

/// Publish a notice to `SystemBroadcast`.
async fn publish_notice(publisher: &Publisher, message: String) {
    if let Err(e) = publisher
        .publish(TopicId::SystemBroadcast, BusEvent::Notice { message })
        .await
    {
        tracing::warn!(error = %e, "failed to publish notice to bus");
    }
}

/// Force an observation cycle regardless of token threshold.
///
/// Loads recent messages, runs the observer, clears recent messages, updates
/// the search index, optionally triggers reflection, and publishes a notice.
pub(super) async fn run_forced_observe(
    mem: &MemorySubsystems<'_>,
    agent: &mut Agent,
    publisher: &Publisher,
) {
    let recent = match load_recent_messages(&mem.layout.recent_messages_json()).await {
        Ok(msgs) => msgs,
        Err(e) => {
            tracing::warn!(error = %e, "forced observe failed to load recent messages");
            publish_notice(publisher, format!("observe failed: {e}")).await;
            return;
        }
    };

    if recent.is_empty() {
        publish_notice(
            publisher,
            "[memory] observe: no recent messages".to_string(),
        )
        .await;
        return;
    }

    let result = match mem.observer.observe(&recent, mem.layout).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "forced observe failed");
            publish_notice(publisher, format!("observe failed: {e}")).await;
            return;
        }
    };

    // Save narrative context if present
    if let Some(narrative) = &result.narrative {
        let ctx = RecentContext {
            narrative: narrative.clone(),
            created_at: crate::time::now_local(mem.observer.timezone()),
            episode_id: result.id.clone(),
        };
        if let Err(e) = save_recent_context(&mem.layout.recent_context_json(), &ctx).await {
            tracing::warn!(error = %e, "failed to save recent context after forced observe");
        }
    }

    if let Err(e) = clear_recent_messages(&mem.layout.recent_messages_json()).await {
        tracing::warn!(error = %e, "failed to clear recent messages after forced observe");
    }
    agent.rotate_messages_after_observation();

    let reflected = finalize_observation(mem, agent, &result).await;

    let suffix = if reflected {
        "; reflection triggered"
    } else {
        ""
    };
    let notice = format!(
        "[memory] observed: {} ({} observations){suffix}",
        result.id, result.observation_count
    );
    publish_notice(publisher, notice).await;
}

/// Mark an episode's `.obs.json` and `.idx.jsonl` as embedded in the manifest.
///
/// If a manifest entry already exists for the file, sets `embedded = true`.
/// If no entry exists, creates one with the file's mtime, empty `doc_ids`, and `embedded = true`.
/// Empty `doc_ids` is safe: the next startup `incremental_sync` will fill them in.
async fn mark_episode_embedded(layout: &WorkspaceLayout, result: &ObserveResult) {
    let manifest_path = layout.index_manifest_json();
    let mut manifest = match IndexManifest::load(&manifest_path).await {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(error = %e, "failed to load manifest to mark embedded");
            return;
        }
    };

    let Some(date_dir) = episode_date_dir(&result.date) else {
        tracing::warn!(date = %result.date, "invalid date format in episode result");
        return;
    };

    let rel_paths = [
        format!("episodes/{date_dir}/{}.obs.json", result.id),
        format!("episodes/{date_dir}/{}.idx.jsonl", result.id),
    ];

    let memory_dir = layout.memory_dir();

    for rel_path in &rel_paths {
        if let Some(entry) = manifest.files.get_mut(rel_path.as_str()) {
            entry.embedded = true;
        } else {
            // File was just created — read mtime from disk
            let abs_path = memory_dir.join(rel_path);
            let mtime = match std::fs::metadata(&abs_path) {
                Ok(meta) => {
                    let modified = meta.modified().unwrap_or(std::time::SystemTime::now());
                    let dt: chrono::DateTime<chrono::Utc> = modified.into();
                    dt.format("%Y-%m-%dT%H:%M:%S").to_string()
                }
                Err(_) => chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S").to_string(),
            };
            manifest.files.insert(
                rel_path.clone(),
                ManifestFileEntry {
                    mtime,
                    doc_ids: Vec::new(),
                    embedded: true,
                },
            );
        }
    }

    if let Err(e) = manifest.save(&manifest_path).await {
        tracing::warn!(error = %e, "failed to save manifest after marking embedded");
    }
}

/// Force a reflection cycle regardless of observation log size.
///
/// Runs the reflector, reloads observations into the agent, and publishes a notice.
pub(super) async fn run_forced_reflect(
    reflector: &Reflector,
    layout: &WorkspaceLayout,
    agent: &mut Agent,
    publisher: &Publisher,
) {
    match reflector.reflect(layout).await {
        Ok(compressed) => {
            if let Err(e) = agent.reload_observations(layout).await {
                tracing::warn!(error = %e, "failed to reload observations after forced reflect");
            }
            publish_notice(
                publisher,
                format!("[memory] reflected: {} observations", compressed.len()),
            )
            .await;
        }
        Err(e) => {
            tracing::warn!(error = %e, "forced reflect failed");
            publish_notice(publisher, format!("reflect failed: {e}")).await;
        }
    }
}
