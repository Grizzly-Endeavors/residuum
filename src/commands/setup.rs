//! Setup subcommand: interactive or flag-driven configuration wizard.

use residuum::checkpoints::CheckpointEngine;
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
pub(super) async fn run_setup_command(args: &SetupArgs) -> Result<(), FatalError> {
    run_setup_command_at(Config::config_dir()?, args).await
}

/// [`run_setup_command`] against an explicit config directory, so the
/// non-interactive (flag-driven) path is testable without touching the real
/// `~/.residuum`.
async fn run_setup_command_at(
    config_dir: std::path::PathBuf,
    args: &SetupArgs,
) -> Result<(), FatalError> {
    use residuum::config::wizard;

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

    let checkpoints = CheckpointEngine::open_for_cli(&config_dir);
    super::checkpoint_config_before_write(
        checkpoints.as_ref(),
        "CLI setup: providers.toml + config.toml".to_string(),
    )
    .await;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn flag_driven_setup_checkpoints_the_config_repo() {
        let dir = tempfile::tempdir().unwrap();

        run_setup_command_at(
            dir.path().to_path_buf(),
            &SetupArgs {
                timezone: Some("UTC".to_string()),
                provider: Some("ollama".to_string()),
                api_key: None,
                model: Some("llama3".to_string()),
                web_search_backend: None,
                web_search_api_key: None,
                web_search_base_url: None,
            },
        )
        .await
        .unwrap();

        assert!(dir.path().join("config.toml").exists());

        let engine = CheckpointEngine::open_for_cli(dir.path()).unwrap();
        let page = engine
            .list_checkpoints(residuum::checkpoints::RepoKind::Config, None, None, None)
            .await
            .unwrap();
        assert_eq!(
            page.items.len(),
            1,
            "the initial bootstrap file should be checkpointed before the wizard overwrites it: {:?}",
            page.items.iter().map(|c| &c.summary).collect::<Vec<_>>()
        );
        assert_eq!(
            page.items.first().unwrap().trigger,
            residuum::checkpoints::CheckpointTrigger::PreConfigWrite
        );
    }

    #[tokio::test]
    async fn setup_refuses_to_overwrite_an_existing_config() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path()).unwrap();
        std::fs::write(dir.path().join("config.toml"), "# existing\n").unwrap();

        run_setup_command_at(
            dir.path().to_path_buf(),
            &SetupArgs {
                timezone: None,
                provider: None,
                api_key: None,
                model: None,
                web_search_backend: None,
                web_search_api_key: None,
                web_search_base_url: None,
            },
        )
        .await
        .unwrap();

        let contents = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
        assert_eq!(
            contents, "# existing\n",
            "existing config must be untouched"
        );
    }
}
