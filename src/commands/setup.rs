//! Setup subcommand: interactive or flag-driven configuration wizard.

use residuum::config::Config;
use residuum::util::FatalError;

#[derive(clap::Args)]
pub(super) struct SetupArgs {
    /// Override the timezone
    #[arg(long)]
    pub timezone: Option<String>,
    /// LLM provider name
    #[arg(long)]
    pub provider: Option<String>,
    /// API key for the LLM provider
    #[arg(long)]
    pub api_key: Option<String>,
    /// Model name to use
    #[arg(long)]
    pub model: Option<String>,
    /// Web search backend (e.g., brave, tavily)
    #[arg(long)]
    pub web_search_backend: Option<String>,
    /// API key for web search
    #[arg(long)]
    pub web_search_api_key: Option<String>,
    /// Base URL for web search API
    #[arg(long)]
    pub web_search_base_url: Option<String>,
}

/// Run the `setup` subcommand — interactive or flag-driven config wizard.
pub(super) fn run_setup_command(args: &SetupArgs) -> Result<(), FatalError> {
    use residuum::config::wizard;

    let config_dir = Config::config_dir()?;
    let config_path = config_dir.join("config.toml");

    if config_path.exists() {
        println!("config.toml already exists at {}", config_path.display());
        println!("edit it directly or delete it to re-run setup");
        return Ok(());
    }

    // Check if any flags are present → non-interactive mode
    let has_flags = args.timezone.is_some()
        || args.provider.is_some()
        || args.api_key.is_some()
        || args.model.is_some()
        || args.web_search_backend.is_some()
        || args.web_search_api_key.is_some()
        || args.web_search_base_url.is_some();

    let answers = if has_flags {
        wizard::from_flags(
            args.timezone.as_deref(),
            args.provider.as_deref(),
            args.api_key.as_deref(),
            args.model.as_deref(),
            args.web_search_backend.as_deref(),
            args.web_search_api_key.as_deref(),
            args.web_search_base_url.as_deref(),
        )?
    } else {
        wizard::run_interactive()?
    };

    // Bootstrap creates the directory + example config
    Config::bootstrap_at_dir(&config_dir)?;
    // Write the wizard-generated config (overwrites the minimal template)
    wizard::write_config(&config_dir, &answers)?;

    // Validate the result
    match Config::load_at(&config_dir) {
        Ok(cfg) => {
            println!("configuration saved to {}", config_path.display());
            println!("  timezone: {}", answers.timezone);
            println!("  model: {}/{}", answers.provider, answers.model);
            if cfg.main.first().and_then(|s| s.api_key.as_ref()).is_some() {
                println!("  api key: configured");
            }
            if let Some(ref backend) = answers.web_search_backend {
                println!("  web search: {backend}");
            }
        }
        Err(err) => {
            println!("warning: config was written but validation failed: {err}");
            println!("you may need to edit {} manually", config_path.display());
        }
    }

    Ok(())
}
