//! Teams management subcommands.

use std::path::{Path, PathBuf};

use clap::Subcommand;

use residuum::interfaces::teams::atk::{forward_redirect, import_atk_project};
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
}
