//! Gateway initialization: builds all subsystems before the event loop starts.

use std::path::Path;
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::actions::store::ActionStore;
use crate::agent::Agent;
use crate::background::BackgroundTaskSpawner;
use crate::background::types::BackgroundResult;
use crate::config::Config;
use crate::error::ResiduumError;
use crate::mcp::SharedMcpRegistry;
use crate::memory::chunk_extractor::read_idx_jsonl;
use crate::memory::observer::Observer;
use crate::memory::recent_messages::load_messages_for_agent;
use crate::memory::reflector::Reflector;
use crate::memory::search::{
    HybridSearcher, MemoryIndex, RebuildResult, create_shared_index, parse_obs_file,
};
use crate::memory::types::IndexManifest;
use crate::memory::vector_store::VectorStore;
use crate::models::{
    CompletionOptions, EmbeddingProvider, HttpClientConfig, SharedHttpClient,
    build_embedding_provider, build_provider_chain,
};
use crate::notify::channels::InboxChannel;
use crate::notify::router::NotificationRouter;
use crate::projects::activation::{ProjectState, SharedProjectState};
use crate::projects::scanner::ProjectIndex;
use crate::skills::{SharedSkillState, SkillIndex, SkillState};
use crate::tools::ToolRegistry;
use crate::workspace::bootstrap::ensure_workspace;
use crate::workspace::identity::IdentityFiles;
use crate::workspace::layout::WorkspaceLayout;

use super::memory::build_memory_components;
use super::spawn_helpers::SpawnContext;

/// All subsystems initialized before the gateway event loop.
pub(super) struct GatewayComponents {
    pub(super) layout: WorkspaceLayout,
    pub(super) tz: chrono_tz::Tz,
    pub(super) agent: Agent,
    pub(super) observer: Observer,
    pub(super) reflector: Reflector,
    pub(super) search_index: Arc<MemoryIndex>,
    #[expect(
        dead_code,
        reason = "used only in tool registration, not read by event loop"
    )]
    pub(super) hybrid_searcher: Arc<HybridSearcher>,
    pub(super) vector_store: Option<Arc<VectorStore>>,
    pub(super) action_store: Arc<tokio::sync::Mutex<ActionStore>>,
    pub(super) action_notify: Arc<tokio::sync::Notify>,
    pub(super) mcp_registry: SharedMcpRegistry,
    pub(super) project_state: SharedProjectState,
    pub(super) skill_state: SharedSkillState,
    pub(super) embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
    pub(super) pulse_enabled: bool,
    pub(super) notification_router: Arc<NotificationRouter>,
    pub(super) http_client: SharedHttpClient,
    pub(super) background_spawner: Arc<BackgroundTaskSpawner>,
    pub(super) background_result_rx: mpsc::Receiver<BackgroundResult>,
    pub(super) spawn_context: Arc<SpawnContext>,
    pub(super) path_policy: crate::tools::SharedPathPolicy,
}

/// Model providers and memory pipeline observers built from config.
pub(super) struct ProviderComponents {
    pub provider: Box<dyn crate::models::ModelProvider>,
    pub observer: Observer,
    pub reflector: Reflector,
    pub embedding_provider: Option<Arc<dyn EmbeddingProvider>>,
}

/// Search index and vector store built during initialization.
pub(super) struct MemoryComponents {
    pub search_index: Arc<MemoryIndex>,
    pub hybrid_searcher: Arc<HybridSearcher>,
    pub vector_store: Option<Arc<VectorStore>>,
}

/// Bootstrap the workspace directory and return the layout and timezone.
///
/// # Errors
/// Returns `ResiduumError` if workspace bootstrapping fails.
pub(super) async fn init_workspace(
    cfg: &Config,
) -> Result<(WorkspaceLayout, chrono_tz::Tz), ResiduumError> {
    let layout = WorkspaceLayout::new(&cfg.workspace_dir);
    let tz = cfg.timezone;
    ensure_workspace(&layout, cfg.name.as_deref(), Some(cfg.timezone.name())).await?;

    std::env::set_current_dir(&cfg.workspace_dir).map_err(|e| {
        ResiduumError::Config(format!(
            "failed to change to workspace directory {}: {e}",
            cfg.workspace_dir.display()
        ))
    })?;
    tracing::info!(workspace = %cfg.workspace_dir.display(), "changed to workspace directory");

    Ok((layout, tz))
}

/// Load identity files and build the shared HTTP client.
///
/// # Errors
/// Returns `ResiduumError` if identity loading or HTTP client construction fails.
pub(super) async fn init_identity_and_http(
    layout: &WorkspaceLayout,
    cfg: &Config,
) -> Result<(IdentityFiles, SharedHttpClient), ResiduumError> {
    let identity = IdentityFiles::load(layout).await?;
    let http = SharedHttpClient::new(&HttpClientConfig::with_timeout(cfg.timeout_secs))
        .map_err(|e| ResiduumError::Config(format!("failed to build HTTP client: {e}")))?;
    Ok((identity, http))
}

/// Build model providers, observer, reflector, and embedding provider.
///
/// # Errors
/// Returns `ResiduumError` if the main model provider fails to build.
pub(super) fn init_providers(
    cfg: &Config,
    tz: chrono_tz::Tz,
    http: SharedHttpClient,
) -> Result<ProviderComponents, ResiduumError> {
    let provider =
        build_provider_chain(&cfg.main, cfg.max_tokens, http.clone(), cfg.retry.clone())?;
    tracing::info!(model = provider.model_name(), "model provider ready");

    let (observer, reflector) = match build_memory_components(cfg, tz, http.clone()) {
        Ok(pair) => pair,
        Err(err) => {
            eprintln!(
                "warning: memory subsystem failed to initialize, running without observer/reflector: {err}"
            );
            tracing::warn!(error = %err, "memory subsystem degraded: observer and reflector disabled");
            (Observer::disabled(tz), Reflector::disabled(tz))
        }
    };

    let embedding_provider: Option<Arc<dyn EmbeddingProvider>> = match cfg
        .embedding
        .as_ref()
        .map(|spec| build_embedding_provider(spec, http, cfg.retry.clone()))
        .transpose()
    {
        Ok(ep) => {
            if let Some(ref e) = ep {
                tracing::info!(model = e.model_name(), "embedding provider ready");
            }
            ep.map(Arc::from)
        }
        Err(err) => {
            eprintln!("warning: embedding provider failed to initialize: {err}");
            tracing::warn!(error = %err, "embedding provider degraded");
            None
        }
    };

    Ok(ProviderComponents {
        provider,
        observer,
        reflector,
        embedding_provider,
    })
}

/// Build the search index, vector store, and hybrid searcher.
///
/// # Errors
/// Returns `ResiduumError` if the search index cannot be created.
pub(super) async fn init_memory(
    cfg: &Config,
    layout: &WorkspaceLayout,
    embedding_provider: Option<&Arc<dyn EmbeddingProvider>>,
) -> Result<MemoryComponents, ResiduumError> {
    // Search index — schema migration + incremental sync
    let manifest_path = layout.index_manifest_json();
    let manifest = match IndexManifest::load(&manifest_path).await {
        Ok(m) => m,
        Err(err) => {
            eprintln!("warning: failed to load index manifest, starting fresh: {err}");
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

    let search_index = match create_shared_index(&layout.search_index_dir()) {
        Ok(idx) => idx,
        Err(err) => {
            eprintln!(
                "warning: failed to create on-disk search index, using in-memory fallback: {err}"
            );
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

/// Initialize all gateway subsystems from config.
///
/// Delegates to `init_workspace`, `init_identity_and_http`, `init_providers`,
/// and `init_memory` for the first stages, then wires up tools, the agent,
/// and remaining subsystems.
///
/// # Errors
/// Returns `ResiduumError` if any subsystem fails to initialize.
#[expect(
    clippy::too_many_lines,
    reason = "sequential initialization pipeline; each section is a distinct setup step"
)]
pub(super) async fn initialize(cfg: &Config) -> Result<GatewayComponents, ResiduumError> {
    let (layout, tz) = init_workspace(cfg).await?;
    let (identity, http) = init_identity_and_http(&layout, cfg).await?;
    let providers = init_providers(cfg, tz, http.clone())?;
    let mem = init_memory(cfg, &layout, providers.embedding_provider.as_ref()).await?;

    // Scheduled actions store
    let actions_path = layout.scheduled_actions_json();
    let action_store = match ActionStore::load(&actions_path).await {
        Ok(store) => Arc::new(tokio::sync::Mutex::new(store)),
        Err(err) => {
            eprintln!(
                "warning: failed to load scheduled actions, starting with empty store: {err}"
            );
            tracing::warn!(error = %err, "action store degraded: starting empty");
            Arc::new(tokio::sync::Mutex::new(ActionStore::new_empty(
                actions_path,
            )))
        }
    };
    let action_notify = Arc::new(tokio::sync::Notify::new());

    // Project + skill state
    let project_index = match ProjectIndex::scan(&layout).await {
        Ok(idx) => idx,
        Err(err) => {
            eprintln!("warning: failed to scan projects, starting with empty index: {err}");
            tracing::warn!(error = %err, "project index degraded: starting empty");
            ProjectIndex::default()
        }
    };
    let project_state: SharedProjectState = Arc::new(tokio::sync::Mutex::new(ProjectState::new(
        project_index,
        layout.clone(),
    )));
    let skill_index = match SkillIndex::scan(&cfg.skills.dirs, None).await {
        Ok(idx) => idx,
        Err(err) => {
            eprintln!("warning: failed to scan skills, starting with empty index: {err}");
            tracing::warn!(error = %err, "skill index degraded: starting empty");
            SkillIndex::default()
        }
    };
    let skill_state: SharedSkillState =
        SkillState::new_shared(skill_index, cfg.skills.dirs.clone());

    // Background task spawner (created before tool registry so tools can hold Arc clones)
    let (bg_result_tx, bg_result_rx) = mpsc::channel::<BackgroundResult>(32);
    let background_spawner = Arc::new(BackgroundTaskSpawner::new(
        bg_result_tx,
        cfg.background.max_concurrent,
        layout.root().to_path_buf(),
        layout.background_dir(),
    ));

    // Clone HTTP client for later use (channel building, reload handler)
    let http_for_channels = http.clone();

    // SpawnContext for pulse/actions/on-demand background task spawning
    let spawn_context = Arc::new(SpawnContext {
        background_config: cfg.background.clone(),
        main_provider_specs: cfg.main.clone(),
        http_client: http,
        max_tokens: cfg.max_tokens,
        retry_config: cfg.retry.clone(),
        identity: identity.clone(),
        options: CompletionOptions {
            max_tokens: Some(cfg.max_tokens),
            temperature: cfg.temperature,
            thinking: cfg.thinking.clone(),
            ..CompletionOptions::default()
        },
        layout: layout.clone(),
        tz,
    });

    // Load workspace MCP servers
    let mcp_registry = crate::mcp::McpRegistry::new_shared();
    match crate::workspace::config::load_mcp_servers(&layout.mcp_json()) {
        Ok(servers) => {
            if !servers.is_empty() {
                let report = mcp_registry
                    .write()
                    .await
                    .reconcile_and_connect(&servers)
                    .await;
                tracing::info!(
                    started = report.started,
                    stopped = report.stopped,
                    failures = report.failures.len(),
                    "workspace MCP servers loaded"
                );
            }
        }
        Err(err) => {
            eprintln!(
                "warning: failed to load mcp.json, starting without workspace MCP servers: {err}"
            );
            tracing::warn!(error = %err, "workspace MCP servers degraded");
        }
    }

    // Load workspace notification channels
    let channel_configs = match crate::workspace::config::load_channel_configs(
        &layout.channels_toml(),
    ) {
        Ok(configs) => configs,
        Err(err) => {
            eprintln!(
                "warning: failed to load channels.toml, starting without external channels: {err}"
            );
            tracing::warn!(error = %err, "workspace channels degraded");
            Vec::new()
        }
    };
    let valid_external_channels: std::collections::HashSet<String> =
        channel_configs.iter().map(|c| c.name.clone()).collect();
    let external_channels = crate::workspace::config::build_external_channels(
        &channel_configs,
        http_for_channels.client(),
    );
    let inbox_channel = InboxChannel::new(layout.inbox_dir(), cfg.timezone);
    let notification_router = Arc::new(NotificationRouter::new(
        external_channels,
        Some(inbox_channel),
    ));

    // Tool registry — block writes to config files (user-managed)
    let mut blocked_paths: Vec<std::path::PathBuf> = vec![
        cfg.config_dir.join("config.toml"),
        cfg.config_dir.join("config.example.toml"),
        cfg.config_dir.join("providers.toml"),
        cfg.config_dir.join("providers.example.toml"),
    ];
    if !cfg.agent.modify_mcp {
        blocked_paths.push(layout.mcp_json());
    }
    if !cfg.agent.modify_channels {
        blocked_paths.push(layout.channels_toml());
    }
    let blocked: std::collections::HashSet<std::path::PathBuf> =
        blocked_paths.into_iter().collect();
    let path_policy =
        crate::tools::PathPolicy::new_shared_with_blocked(layout.root().to_path_buf(), blocked);
    let tool_filter = crate::tools::ToolFilter::new_shared(std::collections::HashSet::new());
    let mut tools = ToolRegistry::new();
    let file_tracker = crate::tools::FileTracker::new_shared();
    tools.register_defaults(file_tracker, Arc::clone(&path_policy));
    tools.register_search_tool(Arc::clone(&mem.hybrid_searcher));
    tools.register_memory_get_tool(layout.episodes_dir());
    tools.register_action_tools(
        Arc::clone(&action_store),
        Arc::clone(&action_notify),
        tz,
        valid_external_channels.clone(),
    );
    let path_policy_for_runtime = Arc::clone(&path_policy);
    tools.register_project_tools(
        Arc::clone(&project_state),
        path_policy,
        Arc::clone(&tool_filter),
        Arc::clone(&mcp_registry),
        Arc::clone(&skill_state),
        tz,
    );
    tools.register_skill_tools(Arc::clone(&skill_state));
    tools.register_inbox_tools(layout.inbox_dir(), layout.inbox_archive_dir(), tz);
    tools.register_background_tools(Arc::clone(&background_spawner));
    tools.register_spawn_tool(
        Arc::clone(&background_spawner),
        Arc::clone(&spawn_context),
        Arc::clone(&project_state),
        Arc::clone(&skill_state),
        Arc::clone(&mcp_registry),
        valid_external_channels,
    );

    tools.register_send_message_tool(Arc::clone(&notification_router), layout.inbox_dir(), tz);

    // Agent
    let options = CompletionOptions {
        max_tokens: Some(cfg.max_tokens),
        temperature: cfg.temperature,
        thinking: cfg.thinking.clone(),
        ..CompletionOptions::default()
    };
    let mut agent = Agent::new(
        providers.provider,
        tools,
        tool_filter,
        Arc::clone(&mcp_registry),
        identity,
        options,
        tz,
        layout.inbox_dir(),
    );
    if let Err(err) = agent.reload_observations(&layout).await {
        eprintln!(
            "warning: failed to load observations, continuing without observation context: {err}"
        );
        tracing::warn!(error = %err, "observation loading degraded");
    }
    if let Err(err) = agent.reload_recent_context(&layout).await {
        eprintln!(
            "warning: failed to load recent context, continuing without recent context: {err}"
        );
        tracing::warn!(error = %err, "recent context loading degraded");
    }

    match load_messages_for_agent(&layout.recent_messages_json()).await {
        Ok(restore) => {
            if !restore.messages.is_empty() {
                tracing::info!(
                    count = restore.messages.len(),
                    "restoring recent messages from previous run"
                );
                agent.restore_messages(restore.messages);
            }
            agent.set_last_user_message_at(restore.last_user_message_at);
        }
        Err(err) => {
            eprintln!(
                "warning: failed to load recent messages, starting with empty history: {err}"
            );
            tracing::warn!(error = %err, "message restore degraded: starting with empty history");
        }
    }

    Ok(GatewayComponents {
        layout,
        tz,
        agent,
        observer: providers.observer,
        reflector: providers.reflector,
        search_index: mem.search_index,
        hybrid_searcher: mem.hybrid_searcher,
        vector_store: mem.vector_store,
        action_store,
        action_notify,
        mcp_registry,
        project_state,
        skill_state,
        embedding_provider: providers.embedding_provider,
        pulse_enabled: cfg.pulse_enabled,
        notification_router,
        http_client: http_for_channels,
        background_spawner,
        background_result_rx: bg_result_rx,
        spawn_context,
        path_policy: path_policy_for_runtime,
    })
}

/// Synchronize the search index (full rebuild or incremental sync).
async fn sync_search_index(
    search_index: &MemoryIndex,
    manifest: &IndexManifest,
    layout: &WorkspaceLayout,
    manifest_path: &std::path::Path,
) {
    if manifest.files.is_empty() {
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
                    eprintln!("warning: failed to save index manifest after rebuild: {save_err}");
                }
            }
            Err(rebuild_err) => eprintln!("warning: failed to rebuild search index: {rebuild_err}"),
        }
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
                    eprintln!("warning: failed to save index manifest after sync: {save_err}");
                }
            }
            Err(sync_err) => {
                eprintln!(
                    "warning: incremental sync failed, falling back to full rebuild: {sync_err}"
                );
                match search_index.rebuild(&layout.memory_dir()) {
                    Ok(result) => {
                        let total = result.obs_count + result.chunk_count;
                        tracing::info!(
                            observations = result.obs_count,
                            chunks = result.chunk_count,
                            "search index rebuilt after sync failure ({total} documents)"
                        );
                        let rebuilt = build_manifest_from_rebuild(result);
                        if let Err(save_err) = rebuilt.save(manifest_path).await {
                            eprintln!(
                                "warning: failed to save index manifest after fallback rebuild: {save_err}"
                            );
                        }
                    }
                    Err(rebuild_err) => {
                        eprintln!("warning: fallback rebuild also failed: {rebuild_err}");
                    }
                }
            }
        }
    }
}

/// Build the vector store, probing for embedding dimensions.
async fn build_vector_store(
    ep: &dyn EmbeddingProvider,
    layout: &WorkspaceLayout,
    manifest: &IndexManifest,
    manifest_path: &std::path::Path,
) -> Option<Arc<VectorStore>> {
    let probe = match ep.embed(&["dimension probe"]).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("warning: embedding dimension probe failed: {e}");
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
            eprintln!("warning: failed to remove old vector store: {e}");
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
                eprintln!("warning: failed to save manifest with embedding info: {e}");
            }

            Some(Arc::new(vs))
        }
        Err(e) => {
            eprintln!("warning: failed to open vector store: {e}");
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
            eprintln!("warning: failed to load manifest for embedding backfill: {e}");
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
                eprintln!("warning: failed to backfill embeddings for {rel_path}: {e}");
                continue;
            }
        } else if rel_path.ends_with(".idx.jsonl") {
            if let Err(e) = backfill_idx_file(vs, ep, &abs_path).await {
                eprintln!("warning: failed to backfill embeddings for {rel_path}: {e}");
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
            eprintln!("warning: failed to save manifest after embedding backfill: {e}");
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
) -> Result<(), ResiduumError> {
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
    let response = ep.embed(&texts).await.map_err(|e| {
        ResiduumError::Memory(format!("embedding failed for {}: {e}", path.display()))
    })?;

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
) -> Result<(), ResiduumError> {
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
    let response = ep.embed(&texts).await.map_err(|e| {
        ResiduumError::Memory(format!("embedding failed for {}: {e}", path.display()))
    })?;

    let embeddings = response.embeddings;
    vs.insert_chunks(&chunks, &embeddings)?;
    Ok(())
}
