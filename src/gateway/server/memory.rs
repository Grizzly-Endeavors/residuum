//! Memory pipeline helpers: observation, reflection, and persistence.

use std::sync::Arc;

use tokio::sync::broadcast;

use crate::agent::Agent;
use crate::config::Config;
use crate::error::IronclawError;
use crate::gateway::protocol::ServerMessage;
use crate::memory::log_store::load_observation_log;
use crate::memory::observer::{ObserveAction, ObserveResult, Observer, ObserverConfig};
use crate::memory::recent_context::{RecentContext, save_recent_context};
use crate::memory::recent_messages::{
    append_recent_messages, clear_recent_messages, load_recent_messages,
};
use crate::memory::reflector::{Reflector, ReflectorConfig};
use crate::memory::search::MemoryIndex;
use crate::memory::types::Visibility;
use crate::memory::vector_store::VectorStore;
use crate::models::{EmbeddingProvider, SharedHttpClient, build_provider_from_provider_spec};
use crate::workspace::layout::WorkspaceLayout;

/// Build observer and reflector from fully-resolved provider specs on `Config`.
///
/// # Errors
/// Returns `IronclawError::Config` if either provider cannot be built.
pub(super) fn build_memory_components(
    cfg: &Config,
    tz: chrono_tz::Tz,
    http: SharedHttpClient,
) -> Result<(Observer, Reflector), IronclawError> {
    let observer_provider = build_provider_from_provider_spec(
        &cfg.observer,
        cfg.max_tokens,
        http.clone(),
        cfg.retry.clone(),
    )?;
    let reflector_provider =
        build_provider_from_provider_spec(&cfg.reflector, cfg.max_tokens, http, cfg.retry.clone())?;

    let observer = Observer::new(
        observer_provider,
        ObserverConfig {
            threshold_tokens: cfg.memory.observer_threshold_tokens,
            cooldown_secs: cfg.memory.observer_cooldown_secs,
            force_threshold_tokens: cfg.memory.observer_force_threshold_tokens,
            tz,
        },
    );

    let reflector = Reflector::new(
        reflector_provider,
        ReflectorConfig {
            threshold_tokens: cfg.memory.reflector_threshold_tokens,
            tz,
        },
    );

    Ok((observer, reflector))
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
        eprintln!("warning: failed to persist recent messages: {e}");
        return ObserveAction::None;
    }

    let recent = match load_recent_messages(&layout.recent_messages_json()).await {
        Ok(msgs) => msgs,
        Err(e) => {
            eprintln!("warning: failed to load recent messages: {e}");
            return ObserveAction::None;
        }
    };

    observer.check_thresholds(&recent)
}

/// Execute an observation cycle: LLM call, clear file, rotate messages, index, reflect, reload.
pub(super) async fn execute_observation(
    observer: &Observer,
    reflector: &Reflector,
    search_index: &MemoryIndex,
    layout: &WorkspaceLayout,
    agent: &mut Agent,
    vector_store: Option<&Arc<VectorStore>>,
    embedding_provider: Option<&Arc<dyn EmbeddingProvider>>,
) {
    let recent = match load_recent_messages(&layout.recent_messages_json()).await {
        Ok(msgs) => msgs,
        Err(e) => {
            eprintln!("warning: failed to load recent messages for observation: {e}");
            return;
        }
    };

    if recent.is_empty() {
        return;
    }

    match observer.observe(&recent, layout).await {
        Ok(result) => {
            tracing::info!(episode_id = %result.id, "observer extracted episode");

            // Save narrative context if present
            if let Some(narrative) = &result.narrative {
                let ctx = RecentContext {
                    narrative: narrative.clone(),
                    created_at: crate::time::now_local(observer.timezone()),
                    episode_id: result.id.clone(),
                };
                if let Err(e) = save_recent_context(&layout.recent_context_json(), &ctx).await {
                    eprintln!("warning: failed to save recent context: {e}");
                }
            }

            if let Err(e) = clear_recent_messages(&layout.recent_messages_json()).await {
                eprintln!("warning: failed to clear recent messages: {e}");
            }
            agent.rotate_messages_after_observation();

            // Index observations and chunks into the search index
            if let Err(e) =
                search_index.index_observations(&result.id, &result.date, &result.observations)
            {
                eprintln!("warning: failed to index observations: {e}");
            }
            if let Err(e) = search_index.index_chunks(&result.chunks) {
                eprintln!("warning: failed to index chunks: {e}");
            }

            // Embed and store in vector index
            embed_observation_result(&result, vector_store, embedding_provider).await;

            run_reflector_if_needed(reflector, layout).await;

            if let Err(e) = agent.reload_observations(layout).await {
                eprintln!("warning: failed to reload observations: {e}");
            }
            if let Err(e) = agent.reload_recent_context(layout).await {
                eprintln!("warning: failed to reload recent context: {e}");
            }
        }
        Err(e) => {
            eprintln!("warning: observer failed: {e}");
        }
    }
}

/// Run the reflector if the observation log exceeds the threshold.
async fn run_reflector_if_needed(reflector: &Reflector, layout: &WorkspaceLayout) {
    let log = match load_observation_log(&layout.observations_json()).await {
        Ok(log) => log,
        Err(e) => {
            eprintln!("warning: failed to load observation log for reflection: {e}");
            return;
        }
    };

    if reflector.should_reflect(&log) {
        match reflector.reflect(layout).await {
            Ok(compressed) => {
                tracing::info!(
                    episodes = compressed.len(),
                    "reflector compressed observation log"
                );
            }
            Err(e) => {
                eprintln!("warning: reflector failed: {e}");
            }
        }
    }
}

/// Embed observations and chunks from an observer result into the vector store.
///
/// Silently returns if no embedding provider or vector store is configured.
/// Embedding failures are reported as warnings, never fatal.
async fn embed_observation_result(
    result: &ObserveResult,
    vector_store: Option<&Arc<VectorStore>>,
    embedding_provider: Option<&Arc<dyn EmbeddingProvider>>,
) {
    let (Some(vs), Some(ep)) = (vector_store, embedding_provider) else {
        return;
    };

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
                    Ok(Err(e)) => eprintln!("warning: failed to insert observation vectors: {e}"),
                    Err(e) => eprintln!("warning: observation vector insert task panicked: {e}"),
                    Ok(Ok(_)) => {}
                }
            }
            Err(e) => {
                eprintln!("warning: failed to embed observations: {e}");
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
                    Ok(Err(e)) => eprintln!("warning: failed to insert chunk vectors: {e}"),
                    Err(e) => eprintln!("warning: chunk vector insert task panicked: {e}"),
                    Ok(Ok(_)) => {}
                }
            }
            Err(e) => {
                eprintln!("warning: failed to embed chunks: {e}");
            }
        }
    }
}

/// Force an observation cycle regardless of token threshold.
///
/// Loads recent messages, runs the observer, clears recent messages, updates
/// the search index, optionally triggers reflection, and broadcasts a `Notice`.
#[expect(
    clippy::too_many_lines,
    reason = "forced observe is a linear pipeline with error handling at each step"
)]
#[expect(
    clippy::too_many_arguments,
    reason = "pipeline function threading subsystem references through from the event loop"
)]
pub(super) async fn run_forced_observe(
    observer: &Observer,
    reflector: &Reflector,
    search_index: &Arc<MemoryIndex>,
    layout: &WorkspaceLayout,
    agent: &mut Agent,
    broadcast_tx: &broadcast::Sender<ServerMessage>,
    vector_store: Option<&Arc<VectorStore>>,
    embedding_provider: Option<&Arc<dyn EmbeddingProvider>>,
) {
    let recent = match load_recent_messages(&layout.recent_messages_json()).await {
        Ok(msgs) => msgs,
        Err(e) => {
            eprintln!("warning: forced observe failed to load recent messages: {e}");
            if broadcast_tx
                .send(ServerMessage::Error {
                    reply_to: None,
                    message: format!("observe failed: {e}"),
                })
                .is_err()
            {
                tracing::trace!("no broadcast receivers for error");
            }
            return;
        }
    };

    if recent.is_empty() {
        if broadcast_tx
            .send(ServerMessage::Notice {
                message: "[memory] observe: no recent messages".to_string(),
            })
            .is_err()
        {
            tracing::trace!("no broadcast receivers for notice");
        }
        return;
    }

    let result = match observer.observe(&recent, layout).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("warning: forced observe failed: {e}");
            if broadcast_tx
                .send(ServerMessage::Error {
                    reply_to: None,
                    message: format!("observe failed: {e}"),
                })
                .is_err()
            {
                tracing::trace!("no broadcast receivers for error");
            }
            return;
        }
    };

    // Save narrative context if present
    if let Some(narrative) = &result.narrative {
        let ctx = RecentContext {
            narrative: narrative.clone(),
            created_at: crate::time::now_local(observer.timezone()),
            episode_id: result.id.clone(),
        };
        if let Err(e) = save_recent_context(&layout.recent_context_json(), &ctx).await {
            eprintln!("warning: failed to save recent context after forced observe: {e}");
        }
    }

    if let Err(e) = clear_recent_messages(&layout.recent_messages_json()).await {
        eprintln!("warning: failed to clear recent messages after forced observe: {e}");
    }
    agent.rotate_messages_after_observation();

    // Index observations and chunks into the search index
    if let Err(e) = search_index.index_observations(&result.id, &result.date, &result.observations)
    {
        eprintln!("warning: failed to index observations after forced observe: {e}");
    }
    if let Err(e) = search_index.index_chunks(&result.chunks) {
        eprintln!("warning: failed to index chunks after forced observe: {e}");
    }

    // Embed and store in vector index
    embed_observation_result(&result, vector_store, embedding_provider).await;

    let reflected = match load_observation_log(&layout.observations_json()).await {
        Ok(log) if reflector.should_reflect(&log) => match reflector.reflect(layout).await {
            Ok(_) => true,
            Err(e) => {
                eprintln!("warning: reflector failed after forced observe: {e}");
                false
            }
        },
        Ok(_) => false,
        Err(e) => {
            eprintln!("warning: failed to load observation log for reflection check: {e}");
            false
        }
    };

    if let Err(e) = agent.reload_observations(layout).await {
        eprintln!("warning: failed to reload observations after forced observe: {e}");
    }
    if let Err(e) = agent.reload_recent_context(layout).await {
        eprintln!("warning: failed to reload recent context after forced observe: {e}");
    }

    let suffix = if reflected {
        "; reflection triggered"
    } else {
        ""
    };
    let notice = format!(
        "[memory] observed: {} ({} observations){suffix}",
        result.id, result.observation_count
    );
    if broadcast_tx
        .send(ServerMessage::Notice { message: notice })
        .is_err()
    {
        tracing::trace!("no broadcast receivers for notice");
    }
}

/// Force a reflection cycle regardless of observation log size.
///
/// Runs the reflector, reloads observations into the agent, and broadcasts a `Notice`.
pub(super) async fn run_forced_reflect(
    reflector: &Reflector,
    layout: &WorkspaceLayout,
    agent: &mut Agent,
    broadcast_tx: &broadcast::Sender<ServerMessage>,
) {
    match reflector.reflect(layout).await {
        Ok(compressed) => {
            if let Err(e) = agent.reload_observations(layout).await {
                eprintln!("warning: failed to reload observations after forced reflect: {e}");
            }
            if broadcast_tx
                .send(ServerMessage::Notice {
                    message: format!("[memory] reflected: {} observations", compressed.len()),
                })
                .is_err()
            {
                tracing::trace!("no broadcast receivers for notice");
            }
        }
        Err(e) => {
            eprintln!("warning: forced reflect failed: {e}");
            if broadcast_tx
                .send(ServerMessage::Error {
                    reply_to: None,
                    message: format!("reflect failed: {e}"),
                })
                .is_err()
            {
                tracing::trace!("no broadcast receivers for error");
            }
        }
    }
}
