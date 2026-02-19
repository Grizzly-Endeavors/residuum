//! `IronClaw`: personal AI agent gateway.
//!
//! Entrypoint that wires up configuration, workspace, model provider,
//! tools, and the interactive REPL loop.

use ironclaw::agent::Agent;
use ironclaw::channels::AgentResponse;
use ironclaw::channels::cli::CliChannel;
use ironclaw::config::{Config, ModelSpec, ProviderKind};
use ironclaw::error::IronclawError;
use ironclaw::memory::observer::{Observer, ObserverConfig};
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

    // Build observer
    let observer = build_observer(&cfg, http)?;

    // Build tool registry
    let mut tools = ToolRegistry::new();
    tools.register_defaults();

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

        match agent.process_message(&msg.content, &cli).await {
            Ok(response) => {
                cli.show_response(&AgentResponse { content: response });
            }
            Err(e) => {
                eprintln!("error: {e}");
            }
        }

        // Run observer if threshold is met
        if observer.should_observe(agent.session()) {
            match observer.observe(agent.session_mut(), &layout).await {
                Ok(episode) => {
                    tracing::info!(
                        episode_id = %episode.id,
                        "observer extracted episode"
                    );
                    // Reload observations so next turn sees the new episode
                    if let Err(e) = agent.reload_observations(&layout).await {
                        eprintln!("warning: failed to reload observations: {e}");
                    }
                }
                Err(e) => {
                    eprintln!("warning: observer failed: {e}");
                }
            }
        }

        println!();
    }

    Ok(())
}

/// Build the observer, using a dedicated model if configured or the main model.
///
/// # Errors
/// Returns `IronclawError::Config` if the observer provider cannot be built.
fn build_observer(cfg: &Config, http: SharedHttpClient) -> Result<Observer, IronclawError> {
    let mem = &cfg.memory;

    let observer_spec = mem.observer_model.as_ref().unwrap_or(&cfg.model);
    let observer_url = mem
        .observer_provider_url
        .as_deref()
        .unwrap_or(&cfg.provider_url);
    let observer_key = mem.observer_api_key.as_deref().or(cfg.api_key.as_deref());

    let provider = build_provider_from_spec(
        observer_spec,
        observer_url,
        observer_key,
        cfg.max_tokens,
        http,
    )?;

    let config = ObserverConfig {
        threshold_tokens: mem.observer_threshold_tokens,
    };

    Ok(Observer::new(provider, config))
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
