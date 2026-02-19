//! `IronClaw`: personal AI agent gateway.
//!
//! Entrypoint that wires up configuration, workspace, model provider,
//! tools, and the interactive REPL loop.

use std::sync::Arc;

use ironclaw::agent::Agent;
use ironclaw::channels::AgentResponse;
use ironclaw::channels::cli::CliChannel;
use ironclaw::config::{Config, ModelSpec, ProviderKind};
use ironclaw::error::IronclawError;
use ironclaw::memory::log_store::load_observation_log;
use ironclaw::memory::observer::{Observer, ObserverConfig};
use ironclaw::memory::recent_store::{
    append_recent_messages, clear_recent_messages, load_recent_messages,
};
use ironclaw::memory::reflector::{Reflector, ReflectorConfig};
use ironclaw::memory::search::create_shared_index;
use ironclaw::models::anthropic::AnthropicClient;
use ironclaw::models::ollama::OllamaClient;
use ironclaw::models::openai::OpenAiClient;
use ironclaw::models::{CompletionOptions, HttpClientConfig, ModelProvider, SharedHttpClient};
use ironclaw::tools::ToolRegistry;
use ironclaw::workspace::bootstrap::ensure_workspace;
use ironclaw::workspace::identity::IdentityFiles;
use ironclaw::workspace::layout::WorkspaceLayout;

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), IronclawError> {
    // Init tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    // Load .env (ignore if missing)
    drop(dotenvy::dotenv());

    // Load config
    let cfg = Config::load()?;
    tracing::info!(
        model = %cfg.model,
        provider_url = %cfg.provider_url,
        workspace = %cfg.workspace_dir.display(),
        "configuration loaded"
    );

    // Ensure workspace
    let layout = WorkspaceLayout::new(&cfg.workspace_dir);
    ensure_workspace(&layout).await?;

    // Change to workspace directory
    std::env::set_current_dir(&cfg.workspace_dir).map_err(|e| {
        IronclawError::Config(format!(
            "failed to change to workspace directory {}: {e}",
            cfg.workspace_dir.display()
        ))
    })?;
    tracing::info!(
        workspace = %cfg.workspace_dir.display(),
        "changed to workspace directory"
    );

    // Load identity files
    let identity = IdentityFiles::load(&layout).await?;

    // Build shared HTTP client
    let http = SharedHttpClient::new(&HttpClientConfig::with_timeout(cfg.timeout_secs))
        .map_err(|e| IronclawError::Config(format!("failed to build HTTP client: {e}")))?;

    // Build model provider
    let provider = build_provider_from_spec(
        &cfg.model,
        &cfg.provider_url,
        cfg.api_key.as_deref(),
        cfg.max_tokens,
        http.clone(),
    )?;
    tracing::info!(model = provider.model_name(), "model provider ready");

    // Build observer and reflector
    let (observer, reflector) = build_memory_components(&cfg, http)?;

    // Build search index
    let search_index = create_shared_index(&layout.search_index_dir())?;
    match search_index.rebuild(&layout.memory_dir()) {
        Ok(count) => tracing::info!(indexed = count, "search index rebuilt"),
        Err(e) => eprintln!("warning: failed to rebuild search index: {e}"),
    }

    // Build tool registry
    let mut tools = ToolRegistry::new();
    tools.register_defaults();
    tools.register_memory_tools(&layout);
    tools.register_search_tool(Arc::clone(&search_index));

    // Build completion options
    let options = CompletionOptions {
        max_tokens: Some(cfg.max_tokens),
    };

    // Build agent
    let mut agent = Agent::new(provider, tools, identity, options);

    // Load observations into agent context
    agent.reload_observations(&layout).await?;

    // Build CLI channel
    let mut cli = CliChannel::new("ironclaw")?;

    println!("IronClaw ready. Type :q or Ctrl+D to exit.\n");

    // REPL loop
    loop {
        let Some(msg) = cli.read_message()? else {
            println!("\nGoodbye!");
            break;
        };

        // Track message count before processing so we can extract new messages
        let before = agent.message_count();

        match agent.process_message(&msg.content, &cli).await {
            Ok(response) => {
                cli.show_response(&AgentResponse { content: response });
            }
            Err(e) => {
                eprintln!("error: {e}");
            }
        }

        // Persist new messages and run memory pipeline
        let new_messages: Vec<_> = agent.messages_since(before).to_vec();
        run_memory_pipeline(
            &new_messages,
            &observer,
            &reflector,
            &search_index,
            &layout,
            &mut agent,
        )
        .await;

        println!();
    }

    Ok(())
}

/// Persist new messages and run the observer/reflector/search pipeline.
async fn run_memory_pipeline(
    new_messages: &[ironclaw::models::Message],
    observer: &Observer,
    reflector: &Reflector,
    search_index: &ironclaw::memory::search::MemoryIndex,
    layout: &WorkspaceLayout,
    agent: &mut Agent,
) {
    // Append new messages to recent_messages.json
    if let Err(e) = append_recent_messages(&layout.recent_messages_json(), new_messages).await {
        eprintln!("warning: failed to persist recent messages: {e}");
        return;
    }

    // Load recent messages and check observer threshold
    let recent = match load_recent_messages(&layout.recent_messages_json()).await {
        Ok(msgs) => msgs,
        Err(e) => {
            eprintln!("warning: failed to load recent messages: {e}");
            return;
        }
    };

    if !observer.should_observe(&recent) {
        return;
    }

    match observer.observe(&recent, layout).await {
        Ok(episode) => {
            tracing::info!(episode_id = %episode.id, "observer extracted episode");

            if let Err(e) = clear_recent_messages(&layout.recent_messages_json()).await {
                eprintln!("warning: failed to clear recent messages: {e}");
            }

            // Index the new episode file
            let episode_path = layout.episodes_dir().join(format!("{}.md", episode.id));
            if let Ok(content) = tokio::fs::read_to_string(&episode_path).await
                && let Err(e) = search_index.index_file(&episode_path.to_string_lossy(), &content)
            {
                eprintln!("warning: failed to index episode: {e}");
            }

            run_reflector_if_needed(reflector, layout).await;

            if let Err(e) = agent.reload_observations(layout).await {
                eprintln!("warning: failed to reload observations: {e}");
            }
        }
        Err(e) => {
            eprintln!("warning: observer failed: {e}");
        }
    }
}

/// Build observer and reflector, sharing the same provider configuration.
///
/// # Errors
/// Returns `IronclawError::Config` if the provider cannot be built.
fn build_memory_components(
    cfg: &Config,
    http: SharedHttpClient,
) -> Result<(Observer, Reflector), IronclawError> {
    let mem = &cfg.memory;

    let spec = mem.observer_model.as_ref().unwrap_or(&cfg.model);
    let url = mem
        .observer_provider_url
        .as_deref()
        .unwrap_or(&cfg.provider_url);
    let key = mem.observer_api_key.as_deref().or(cfg.api_key.as_deref());

    let observer_provider = build_provider_from_spec(spec, url, key, cfg.max_tokens, http.clone())?;
    let reflector_provider = build_provider_from_spec(spec, url, key, cfg.max_tokens, http)?;

    let observer = Observer::new(
        observer_provider,
        ObserverConfig {
            threshold_tokens: mem.observer_threshold_tokens,
        },
    );

    let reflector = Reflector::new(
        reflector_provider,
        ReflectorConfig {
            threshold_tokens: mem.reflector_threshold_tokens,
        },
    );

    Ok((observer, reflector))
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

/// Build a model provider from explicit parameters.
///
/// # Errors
/// Returns `IronclawError::Config` if the API key is missing for providers
/// that require it.
fn build_provider_from_spec(
    spec: &ModelSpec,
    url: &str,
    api_key: Option<&str>,
    max_tokens: u32,
    http: SharedHttpClient,
) -> Result<Box<dyn ModelProvider>, IronclawError> {
    match spec.kind {
        ProviderKind::Anthropic => {
            let key = api_key.ok_or_else(|| {
                IronclawError::Config(
                    "anthropic requires an API key (set ANTHROPIC_API_KEY or api_key in config)"
                        .to_string(),
                )
            })?;

            Ok(Box::new(AnthropicClient::new(
                http,
                url,
                key,
                &spec.model,
                max_tokens,
            )))
        }
        ProviderKind::Ollama => Ok(Box::new(OllamaClient::with_http_client(
            http,
            url,
            &spec.model,
        ))),
        ProviderKind::OpenAi => {
            if let Some(key) = api_key {
                Ok(Box::new(OpenAiClient::with_http_client_and_api_key(
                    http,
                    url,
                    &spec.model,
                    key,
                )))
            } else {
                Ok(Box::new(OpenAiClient::with_http_client(
                    http,
                    url,
                    &spec.model,
                )))
            }
        }
    }
}
