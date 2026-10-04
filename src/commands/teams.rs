//! Teams management subcommands.

use std::path::{Path, PathBuf};

use clap::Subcommand;

use residuum::interfaces::teams::atk::{
    AtkScaffoldOptions, derive_cloud_teams_endpoint, forward_redirect, import_atk_project,
    resolve_atk_paths, scaffold_atk_project,
};
use residuum::util::FatalError;

/// Teams management subcommands.
#[derive(Subcommand)]
pub(super) enum TeamsCommand {
    /// Import credentials and configuration from an ATK project
    ImportAtk {
        /// Name of the agent to configure
        #[arg(long)]
        agent: String,
        /// Path to the teams-app project directory (defaults to <agent>/teams-app)
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// Forward an OAuth redirect URL to the local ATK login listener
    ForwardRedirect {
        /// The redirect URL copied from your browser (e.g. `http://localhost:<port>/?code=...`)
        url: String,
    },
    /// Scaffold a new Microsoft 365 Agents Toolkit project for an agent
    AtkScaffold(Box<AtkScaffoldArgs>),
    /// Inspect ATK project paths for an agent
    AtkPaths {
        /// Name of the agent
        #[arg(long)]
        agent: String,
        /// Output summary as JSON
        #[arg(long)]
        json: bool,
    },
}

/// Arguments for `residuum teams atk-scaffold`.
#[derive(clap::Args, Debug, Clone)]
pub(super) struct AtkScaffoldArgs {
    /// Name of the agent to configure
    #[arg(long)]
    pub agent: String,

    /// Messaging endpoint URL for Microsoft Bot Framework.
    /// If omitted and Residuum Cloud is connected, defaults to the derived cloud endpoint.
    #[arg(long)]
    pub endpoint: Option<String>,

    /// Path to the teams-app project directory (defaults to <agent>/teams-app)
    #[arg(long)]
    pub dir: Option<PathBuf>,

    /// Overwrite existing files in the project directory
    #[arg(long)]
    pub force: bool,

    /// Bot display name (defaults to agent name)
    #[arg(long)]
    pub bot_name: Option<String>,

    /// Developer name in manifest (defaults to "Residuum")
    #[arg(long)]
    pub developer_name: Option<String>,

    /// Developer website URL in manifest
    #[arg(long)]
    pub developer_url: Option<String>,

    /// Privacy statement URL in manifest
    #[arg(long)]
    pub privacy_url: Option<String>,

    /// Terms of use URL in manifest
    #[arg(long)]
    pub terms_url: Option<String>,

    /// Short description of the bot
    #[arg(long)]
    pub short_description: Option<String>,

    /// Long description of the bot
    #[arg(long)]
    pub long_description: Option<String>,

    /// Path to custom 192x192 PNG color icon
    #[arg(long)]
    pub color_icon: Option<PathBuf>,

    /// Path to custom 32x32 PNG outline icon
    #[arg(long)]
    pub outline_icon: Option<PathBuf>,

    /// Output summary as JSON
    #[arg(long)]
    pub json: bool,
}

/// Run the `teams` subcommand.
pub(super) async fn run_teams_command(command: &TeamsCommand) -> Result<(), FatalError> {
    let residuum_root = residuum::config::residuum_root()?;
    run_teams_command_at(&residuum_root, command).await
}

/// Run the `teams` subcommand against a specific residuum root directory (for testing).
pub(super) async fn run_teams_command_at(
    residuum_root: &Path,
    command: &TeamsCommand,
) -> Result<(), FatalError> {
    match command {
        TeamsCommand::ImportAtk { agent, dir } => {
            let project_dir = dir
                .clone()
                .unwrap_or_else(|| residuum_root.join(agent).join("teams-app"));
            let result = import_atk_project(&project_dir, agent, residuum_root).await?;
            println!("Teams integration configured for agent '{agent}':");
            println!("  Bot ID:        {}", result.bot_id);
            println!("  Tenant ID:     {}", result.tenant_id);
            if let Some(ref app_id) = result.teams_app_id {
                println!("  Teams App ID:  {app_id}");
            }
            println!("  Bot Password:  stored encrypted in secret 'teams'");
            println!(
                "  Config:        updated [teams] in {}/{agent}/config.toml",
                residuum_root.display()
            );
        }
        TeamsCommand::ForwardRedirect { url } => {
            let status = forward_redirect(url).await?;
            println!("Successfully forwarded redirect URL to local listener (HTTP {status}).");
        }
        TeamsCommand::AtkScaffold(args) => {
            let endpoint = match args.endpoint.clone() {
                Some(ep) => ep,
                None => {
                    if let Some(derived) =
                        derive_cloud_teams_endpoint(residuum_root, &args.agent).await
                    {
                        derived
                    } else {
                        return Err(FatalError::Config(
                            "no --endpoint was specified and could not derive a cloud endpoint (is Residuum Cloud connected?); please provide --endpoint <URL>".to_string()
                        ));
                    }
                }
            };
            let options = AtkScaffoldOptions {
                agent_name: args.agent.clone(),
                endpoint,
                project_dir: args.dir.clone(),
                force: args.force,
                bot_name: args.bot_name.clone(),
                developer_name: args.developer_name.clone(),
                developer_url: args.developer_url.clone(),
                privacy_url: args.privacy_url.clone(),
                terms_url: args.terms_url.clone(),
                short_description: args.short_description.clone(),
                long_description: args.long_description.clone(),
                color_icon: args.color_icon.clone(),
                outline_icon: args.outline_icon.clone(),
            };
            let result = scaffold_atk_project(residuum_root, &options).await?;
            if args.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&result)
                        .map_err(|e| FatalError::Config(e.to_string()))?
                );
            } else {
                println!("Teams ATK project scaffolded successfully:");
                println!("  Agent:        {}", args.agent);
                println!("  Project Dir:  {}", result.project_dir.display());
                println!("  ATK Binary:   {}", result.atk_bin.display());
                println!("  Package Zip:  {}", result.package_zip.display());
                println!("  Env File:     {}", result.env_file.display());
                println!("  Endpoint:     {}", result.endpoint);
            }
        }
        TeamsCommand::AtkPaths { agent, json } => {
            let paths = resolve_atk_paths(residuum_root, agent);
            if *json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&paths)
                        .map_err(|e| FatalError::Config(e.to_string()))?
                );
            } else {
                println!("Teams ATK paths for agent '{agent}':");
                println!("  Project Dir:  {}", paths.project_dir.display());
                println!("  ATK Binary:   {}", paths.atk_bin.display());
                println!("  Package Zip:  {}", paths.package_zip.display());
                println!("  Env File:     {}", paths.env_file.display());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn run_teams_command_import_atk() {
        let temp = tempfile::tempdir().unwrap();
        let residuum_root = temp.path().to_path_buf();
        let hub_path = residuum_root.join("hub");
        let agent_path = residuum_root.join("scout");

        tokio::fs::create_dir_all(&hub_path).await.unwrap();
        tokio::fs::create_dir_all(&agent_path).await.unwrap();

        tokio::fs::write(hub_path.join("config.toml"), "timezone = \"UTC\"\n")
            .await
            .unwrap();

        tokio::fs::write(
            agent_path.join("config.toml"),
            "[agent]\nname = \"scout\"\n",
        )
        .await
        .unwrap();

        let teams_app = agent_path.join("teams-app");
        let env_dir = teams_app.join("env");
        tokio::fs::create_dir_all(&env_dir).await.unwrap();

        tokio::fs::write(
            env_dir.join(".env.residuum"),
            "BOT_ID=bot-id-123\nTEAMS_APP_TENANT_ID=tenant-id-456\n",
        )
        .await
        .unwrap();

        tokio::fs::write(
            env_dir.join(".env.residuum.user"),
            "SECRET_BOT_PASSWORD=plain-text-legacy-pw\n",
        )
        .await
        .unwrap();

        let cmd = TeamsCommand::ImportAtk {
            agent: "scout".to_string(),
            dir: None,
        };

        run_teams_command_at(&residuum_root, &cmd).await.unwrap();

        let store = residuum::config::SecretStore::load(&hub_path).unwrap();
        assert_eq!(store.get("teams"), Some("plain-text-legacy-pw"));

        let patched = tokio::fs::read_to_string(agent_path.join("config.toml"))
            .await
            .unwrap();
        assert!(patched.contains("[teams]"));
        assert!(patched.contains("app_id = \"bot-id-123\""));
        assert!(patched.contains("tenant_id = \"tenant-id-456\""));
        assert!(patched.contains("app_password = \"secret:teams\""));
    }

    #[tokio::test]
    async fn run_teams_command_atk_scaffold_and_paths() {
        let temp = tempfile::tempdir().unwrap();
        let residuum_root = temp.path().to_path_buf();
        let agent_path = residuum_root.join("scout");
        tokio::fs::create_dir_all(&agent_path).await.unwrap();

        let scaffold_cmd = TeamsCommand::AtkScaffold(Box::new(AtkScaffoldArgs {
            agent: "scout".to_string(),
            endpoint: Some("https://example.com/teams".to_string()),
            dir: None,
            force: false,
            bot_name: Some("Scout Bot".to_string()),
            developer_name: None,
            developer_url: None,
            privacy_url: None,
            terms_url: None,
            short_description: None,
            long_description: None,
            color_icon: None,
            outline_icon: None,
            json: true,
        }));

        run_teams_command_at(&residuum_root, &scaffold_cmd)
            .await
            .unwrap();

        let teams_app = agent_path.join("teams-app");
        assert!(teams_app.join("m365agents.yml").exists());
        assert!(teams_app.join("appPackage/manifest.json").exists());
        assert!(teams_app.join("env/.env.residuum").exists());

        let paths_cmd = TeamsCommand::AtkPaths {
            agent: "scout".to_string(),
            json: true,
        };

        run_teams_command_at(&residuum_root, &paths_cmd)
            .await
            .unwrap();
    }
}
