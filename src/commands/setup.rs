//! Setup subcommand: interactive or flag-driven configuration wizard.

use residuum::checkpoints::CheckpointEngine;
use residuum::config::Config;
use residuum::util::FatalError;

#[derive(clap::Args)]
pub(super) struct SetupArgs {
    /// What the agent should call you (written to the team's USER.md)
    #[arg(long)]
    pub user_name: Option<String>,
    /// Name of the first agent (defaults to "assistant")
    #[arg(long)]
    pub agent_name: Option<String>,
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
    run_setup_command_at(residuum::config::residuum_root()?, args).await
}

/// [`run_setup_command`] against an explicit `~/.residuum` root, so the
/// non-interactive (flag-driven) path is testable without touching the real
/// `~/.residuum`.
async fn run_setup_command_at(
    residuum_root: std::path::PathBuf,
    args: &SetupArgs,
) -> Result<(), FatalError> {
    use residuum::config::{HubConfig, wizard};

    let hub_dir = residuum::config::paths::hub_dir(&residuum_root);
    let hub_config_path = hub_dir.join("config.toml");

    if hub_config_path.exists() {
        println!("hub config already exists at {}", hub_config_path.display());
        println!("edit it directly or delete it to re-run setup");
        return Ok(());
    }

    // Check if any flags are present → non-interactive mode
    let has_flags = args.user_name.is_some()
        || args.agent_name.is_some()
        || args.timezone.is_some()
        || args.provider.is_some()
        || args.api_key.is_some()
        || args.model.is_some()
        || args.web_search_backend.is_some()
        || args.web_search_api_key.is_some()
        || args.web_search_base_url.is_some();

    let answers = if has_flags {
        wizard::from_flags(&wizard::WizardFlags {
            user_name: args.user_name.as_deref(),
            agent_name: args.agent_name.as_deref(),
            timezone: args.timezone.as_deref(),
            provider: args.provider.as_deref(),
            api_key: args.api_key.as_deref(),
            model: args.model.as_deref(),
            web_search_backend: args.web_search_backend.as_deref(),
            web_search_api_key: args.web_search_api_key.as_deref(),
            web_search_base_url: args.web_search_base_url.as_deref(),
        })?
    } else {
        wizard::run_interactive()?
    };

    // Bootstrap creates the hub directory + example config
    HubConfig::bootstrap_at(&hub_dir)?;

    let checkpoints = CheckpointEngine::open_for_cli(&hub_dir);
    super::checkpoint_config_before_write(
        checkpoints.as_ref(),
        "CLI setup: hub config.toml".to_string(),
    )
    .await;

    // Write the hub config and the first agent's workspace and config
    wizard::write_config(&residuum_root, &answers).await?;

    // Validate the result
    let agent_dir = residuum_root.join(&answers.agent_name);
    let loaded =
        HubConfig::load_at(&hub_dir).and_then(|hub| Config::load_agent_at(&agent_dir, &hub));
    match loaded {
        Ok(cfg) => {
            println!("configuration saved under {}", residuum_root.display());
            println!("  agent: {}", answers.agent_name);
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
            println!(
                "you may need to edit {} and {} manually",
                hub_config_path.display(),
                agent_dir.join("config").display()
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flag_args(agent_name: Option<&str>, user_name: Option<&str>) -> SetupArgs {
        SetupArgs {
            user_name: user_name.map(ToString::to_string),
            agent_name: agent_name.map(ToString::to_string),
            timezone: Some("UTC".to_string()),
            provider: Some("ollama".to_string()),
            api_key: None,
            model: Some("llama3".to_string()),
            web_search_backend: None,
            web_search_api_key: None,
            web_search_base_url: None,
        }
    }

    #[tokio::test]
    async fn flag_driven_setup_writes_the_hub_and_agent_layout() {
        let dir = tempfile::tempdir().unwrap();

        run_setup_command_at(
            dir.path().to_path_buf(),
            &flag_args(Some("scout"), Some("Sam")),
        )
        .await
        .unwrap();

        assert!(dir.path().join("hub/config.toml").exists());
        assert!(dir.path().join("scout/config/config.toml").exists());
        assert!(dir.path().join("scout/config/providers.toml").exists());
        let team =
            residuum::config::paths::TeamPaths::new(residuum::config::paths::team_dir(dir.path()));
        let user_md = std::fs::read_to_string(team.user_md()).unwrap();
        assert!(
            user_md.contains("Sam"),
            "team USER.md should carry the user's name: {user_md}"
        );
        let soul = std::fs::read_to_string(dir.path().join("scout/SOUL.md")).unwrap();
        assert!(
            soul.contains("**Name**: scout"),
            "SOUL.md should carry the agent's name: {soul}"
        );
        assert!(!soul.contains("Ralph"));
    }

    #[tokio::test]
    async fn flag_driven_setup_checkpoints_the_hub_config_repo() {
        let dir = tempfile::tempdir().unwrap();

        run_setup_command_at(dir.path().to_path_buf(), &flag_args(None, None))
            .await
            .unwrap();

        let engine = CheckpointEngine::open_for_cli(&dir.path().join("hub")).unwrap();
        let page = engine
            .list_checkpoints(residuum::checkpoints::RepoKind::Hub, None, None, None, None)
            .await
            .unwrap();
        assert_eq!(
            page.items.len(),
            1,
            "the setup run should checkpoint the hub config repo once: {:?}",
            page.items.iter().map(|c| &c.summary).collect::<Vec<_>>()
        );
        assert_eq!(
            page.items.first().unwrap().trigger,
            residuum::checkpoints::CheckpointTrigger::PreConfigWrite
        );
    }

    #[tokio::test]
    async fn setup_rejects_a_reserved_agent_name() {
        let dir = tempfile::tempdir().unwrap();

        let result =
            run_setup_command_at(dir.path().to_path_buf(), &flag_args(Some("hub"), None)).await;

        assert!(result.is_err(), "'hub' is reserved and must be refused");
        assert!(!dir.path().join("hub/config.toml").exists());
    }

    #[tokio::test]
    async fn setup_refuses_to_overwrite_an_existing_hub_config() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("hub")).unwrap();
        std::fs::write(dir.path().join("hub/config.toml"), "# existing\n").unwrap();

        run_setup_command_at(dir.path().to_path_buf(), &flag_args(None, None))
            .await
            .unwrap();

        let contents = std::fs::read_to_string(dir.path().join("hub/config.toml")).unwrap();
        assert_eq!(
            contents, "# existing\n",
            "existing config must be untouched"
        );
    }
}
