//! `IronClaw`: personal AI agent gateway.
//!
//! Entrypoint that wires up configuration, workspace, model provider,
//! tools, and the interactive REPL loop.

use ironclaw::agent::Agent;
use ironclaw::channels::AgentResponse;
use ironclaw::channels::cli::CliChannel;
use ironclaw::config::{Config, ProviderKind};
use ironclaw::error::IronclawError;
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
    let provider: Box<dyn ModelProvider> = build_provider(&cfg, http)?;
    tracing::info!(model = provider.model_name(), "model provider ready");

    // Build tool registry
    let mut tools = ToolRegistry::new();
    tools.register_defaults();

    // Build completion options
    let options = CompletionOptions {
        max_tokens: Some(cfg.max_tokens),
    };

    // Build agent
    let mut agent = Agent::new(provider, tools, identity, options);

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

        println!();
    }

    Ok(())
}

/// Build the appropriate model provider based on config.
///
/// # Errors
/// Returns `IronclawError::Config` if the API key is missing for providers
/// that require it.
fn build_provider(
    cfg: &Config,
    http: SharedHttpClient,
) -> Result<Box<dyn ModelProvider>, IronclawError> {
    match cfg.model.kind {
        ProviderKind::Anthropic => {
            let api_key = cfg.api_key.as_ref().ok_or_else(|| {
                IronclawError::Config(
                    "anthropic requires an API key (set ANTHROPIC_API_KEY or api_key in config)"
                        .to_string(),
                )
            })?;

            Ok(Box::new(AnthropicClient::new(
                http,
                &cfg.provider_url,
                api_key,
                &cfg.model.model,
                cfg.max_tokens,
            )))
        }
        ProviderKind::Ollama => Ok(Box::new(OllamaClient::with_http_client(
            http,
            &cfg.provider_url,
            &cfg.model.model,
        ))),
        ProviderKind::OpenAi => {
            if let Some(api_key) = &cfg.api_key {
                Ok(Box::new(OpenAiClient::with_http_client_and_api_key(
                    http,
                    &cfg.provider_url,
                    &cfg.model.model,
                    api_key,
                )))
            } else {
                Ok(Box::new(OpenAiClient::with_http_client(
                    http,
                    &cfg.provider_url,
                    &cfg.model.model,
                )))
            }
        }
    }
}
