//! Teams management subcommands.

use std::path::{Path, PathBuf};

use clap::Subcommand;

use residuum::interfaces::teams::atk::{
    AtkScaffoldOptions, derive_cloud_teams_endpoint, forward_redirect, import_atk_project,
    resolve_atk_paths, scaffold_atk_project,
};
use residuum::interfaces::teams::atk_runner::AgentLoginManager;
use residuum::interfaces::teams::setup_job::get_or_create_manager;
use residuum::interfaces::teams::setup_types::CleanupRequest;
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
    /// Manage detached Microsoft 365 Agents Toolkit authentication
    AtkLogin {
        /// Name of the agent to authenticate
        #[arg(long)]
        agent: String,
        /// Check authentication status for this agent
        #[arg(long)]
        status: bool,
        /// Cancel a running detached login process
        #[arg(long)]
        cancel: bool,
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
    /// Clean up local setup files, CLI installation, and/or Microsoft 365 sign-out
    Cleanup {
        /// Name of the agent to clean up
        #[arg(long)]
        agent: String,
        /// Remove local teams-app project files and credentials
        #[arg(long)]
        project_files: bool,
        /// Remove hub-level m365agentstoolkit CLI installation
        #[arg(long)]
        cli: bool,
        /// Sign out of Microsoft 365 account via ATK CLI
        #[arg(long)]
        sign_out: bool,
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
        TeamsCommand::AtkLogin {
            agent,
            status,
            cancel,
        } => {
            handle_atk_login(residuum_root, agent, *status, *cancel).await?;
        }
        TeamsCommand::AtkScaffold(args) => {
            handle_atk_scaffold(residuum_root, args).await?;
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
        TeamsCommand::Cleanup {
            agent,
            project_files,
            cli,
            sign_out,
            json,
        } => {
            let req = CleanupRequest {
                project_files: *project_files,
                cli: *cli,
                sign_out: *sign_out,
            };
            handle_cleanup(residuum_root, agent, req, *json).await?;
        }
    }
    Ok(())
}

async fn handle_atk_login(
    residuum_root: &Path,
    agent: &str,
    status: bool,
    cancel: bool,
) -> Result<(), FatalError> {
    if cancel {
        let cancelled = AgentLoginManager::cancel(residuum_root, agent).await?;
        if cancelled {
            println!("Login process cancelled for agent '{agent}'.");
        } else {
            println!("No active login process found for agent '{agent}'.");
        }
    } else if status {
        let status_msg = AgentLoginManager::status(residuum_root, agent).await;
        println!("Microsoft 365 sign-in status for agent '{agent}':\n{status_msg}");
    } else {
        let (url, port) = AgentLoginManager::start(residuum_root, agent).await?;
        println!("Microsoft 365 login started in background for agent '{agent}'.");
        println!("Open this URL in your browser to sign in:");
        println!("  {url}");
        println!("Redirect port: {port}");
        println!("Once signed in, if remote, forward redirect with:");
        println!("  residuum teams forward-redirect \"<pasted-url>\"");
    }
    Ok(())
}

async fn handle_atk_scaffold(
    residuum_root: &Path,
    args: &AtkScaffoldArgs,
) -> Result<(), FatalError> {
    let endpoint = match args.endpoint.clone() {
        Some(ep) => ep,
        None => {
            if let Some(derived) = derive_cloud_teams_endpoint(residuum_root, &args.agent).await {
                derived
            } else {
                return Err(FatalError::Config(
                    "no --endpoint was specified and could not derive a cloud endpoint (is Residuum Cloud connected?); please provide --endpoint <URL>".to_string(),
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
            serde_json::to_string_pretty(&result).map_err(|e| FatalError::Config(e.to_string()))?
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
    Ok(())
}

async fn handle_cleanup(
    residuum_root: &Path,
    agent: &str,
    req: CleanupRequest,
    json: bool,
) -> Result<(), FatalError> {
    if !req.project_files && !req.cli && !req.sign_out {
        return Err(FatalError::Config(
            "no cleanup targets specified; provide at least one of --project-files, --cli, or --sign-out".to_string(),
        ));
    }

    let manager = get_or_create_manager(residuum_root);
    let result = manager.cleanup(agent, req).await;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&result).map_err(|e| FatalError::Config(e.to_string()))?
        );
    } else {
        println!("Teams cleanup completed for agent '{agent}':");
        if !result.removed.is_empty() {
            println!("  Removed: {}", result.removed.join(", "));
        }
        if !result.failed.is_empty() {
            println!("  Failures:");
            for failure in &result.failed {
                println!("    - {}: {}", failure.item, failure.message);
            }
        }
    }
    if !result.failed.is_empty() {
        return Err(FatalError::Other(anyhow::anyhow!(
            "one or more cleanup tasks failed"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn run_teams_command_cleanup() {
        let temp = tempfile::tempdir().unwrap();
        let residuum_root = temp.path().to_path_buf();
        let agent_path = residuum_root.join("scout");
        let teams_app = agent_path.join("teams-app");
        let cli_dir = residuum_root.join("hub/tools/m365agentstoolkit");
        tokio::fs::create_dir_all(&teams_app).await.unwrap();
        tokio::fs::create_dir_all(&cli_dir).await.unwrap();

        // Error when no flags specified
        let empty_cmd = TeamsCommand::Cleanup {
            agent: "scout".to_string(),
            project_files: false,
            cli: false,
            sign_out: false,
            json: false,
        };
        assert!(
            run_teams_command_at(&residuum_root, &empty_cmd)
                .await
                .is_err()
        );

        // Success when cleanup flags specified
        let cmd = TeamsCommand::Cleanup {
            agent: "scout".to_string(),
            project_files: true,
            cli: true,
            sign_out: false,
            json: true,
        };
        run_teams_command_at(&residuum_root, &cmd).await.unwrap();
        assert!(!teams_app.exists());
        assert!(!cli_dir.exists());
    }

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

    #[tokio::test]
    async fn run_teams_command_atk_login_status_and_cancel_handling() {
        let temp = tempfile::tempdir().unwrap();
        let residuum_root = temp.path().to_path_buf();
        let agent_path = residuum_root.join("scout");
        let teams_app = agent_path.join("teams-app");
        tokio::fs::create_dir_all(&teams_app).await.unwrap();

        // 1. Status when no pid file exists
        let status_cmd = TeamsCommand::AtkLogin {
            agent: "scout".to_string(),
            status: true,
            cancel: false,
        };
        run_teams_command_at(&residuum_root, &status_cmd)
            .await
            .unwrap();

        // 2. Status with stale pid file (process dead)
        let pid_path = teams_app.join("atk-login.pid");
        let log_path = teams_app.join("atk-login.log");
        tokio::fs::write(&pid_path, "9999999\n").await.unwrap();
        tokio::fs::write(&log_path, "Login timed out\n")
            .await
            .unwrap();

        let status_str = AgentLoginManager::status(&residuum_root, "scout").await;
        assert!(status_str.contains("failed (process exited)"));
        assert!(status_str.contains("Login timed out"));

        // 3. Cancel command removes pid file
        let cancel_cmd = TeamsCommand::AtkLogin {
            agent: "scout".to_string(),
            status: false,
            cancel: true,
        };
        run_teams_command_at(&residuum_root, &cancel_cmd)
            .await
            .unwrap();
        assert!(!pid_path.exists());
    }
}
