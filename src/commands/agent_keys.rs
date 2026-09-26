//! `agent-keys` subcommand: manage credentials the agent can use in commands.

use std::io::Read as _;
use std::path::PathBuf;

use residuum::agent_keys::{AgentKeys, KeyCreator};
use residuum::checkpoints::CheckpointEngine;
use residuum::config::Config;
use residuum::util::FatalError;

/// Agent key management subcommands.
#[derive(clap::Subcommand)]
pub(super) enum AgentKeysCommand {
    /// Store a key the agent can use (prompts for the value if omitted)
    Set {
        /// Key name: lowercase letters, digits, underscores; exposed to
        /// commands as an environment variable named in uppercase
        name: String,
        /// Value to store (prompted with masked input if omitted)
        value: Option<String>,
        /// Read the value from stdin instead of prompting
        #[arg(long, conflicts_with = "value")]
        stdin: bool,
        /// What the key is and what it grants, shown to the agent
        #[arg(long, short)]
        description: Option<String>,
    },
    /// List keys (names, environment variables, creators, descriptions)
    List,
    /// Remove a key
    Delete {
        /// Name of the key to remove
        name: String,
    },
}

/// Run the `agent-keys` subcommand.
pub(super) async fn run_agent_keys_command(command: &AgentKeysCommand) -> Result<(), FatalError> {
    run_agent_keys_command_at(Config::config_dir()?, command).await
}

/// [`run_agent_keys_command`] against an explicit config directory, so the
/// dispatch logic is testable without touching the real `~/.residuum`.
async fn run_agent_keys_command_at(
    config_dir: PathBuf,
    command: &AgentKeysCommand,
) -> Result<(), FatalError> {
    let keys = AgentKeys::new(config_dir.clone());
    let checkpoints = CheckpointEngine::open_for_cli(&config_dir);

    match command {
        AgentKeysCommand::Set {
            name,
            value,
            stdin,
            description,
        } => {
            let resolved = read_value(name, value.as_deref(), *stdin)?;
            super::checkpoint_config_before_write(
                checkpoints.as_ref(),
                format!("CLI set agent key '{name}'"),
            )
            .await;
            let warning = keys
                .set(name, &resolved, description.as_deref(), KeyCreator::User)
                .await
                .map_err(|e| FatalError::Config(format!("couldn't store agent key: {e}")))?;
            println!(
                "agent key '{name}' saved; commands that name it get ${}",
                residuum::agent_keys::env_var_for(name)
            );
            if let Some(warning) = warning {
                println!("warning: {warning}");
            }
        }
        AgentKeysCommand::List => {
            let snapshot = keys
                .snapshot()
                .await
                .map_err(|e| FatalError::Config(format!("couldn't read agent keys: {e}")))?;
            let list = snapshot.store.list();
            if list.is_empty() {
                println!("no agent keys stored");
            }
            for key in list {
                let description = if key.description.is_empty() {
                    String::new()
                } else {
                    format!("  {}", key.description)
                };
                println!(
                    "{}  ${}  ({}){description}",
                    key.name,
                    key.env_var,
                    key.created_by.as_str()
                );
            }
        }
        AgentKeysCommand::Delete { name } => {
            super::checkpoint_config_before_write(
                checkpoints.as_ref(),
                format!("CLI delete agent key '{name}'"),
            )
            .await;
            keys.delete(name, KeyCreator::User)
                .await
                .map_err(|e| FatalError::Config(format!("couldn't delete agent key: {e}")))?;
            println!("agent key '{name}' deleted");
        }
    }

    Ok(())
}

/// The value from the argument, stdin, or a masked prompt, in that order.
fn read_value(name: &str, value: Option<&str>, stdin: bool) -> Result<String, FatalError> {
    if let Some(v) = value {
        return Ok(v.to_string());
    }
    if stdin {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| FatalError::Config(format!("failed to read value from stdin: {e}")))?;
        return Ok(buf.trim_end_matches(['\r', '\n']).to_string());
    }
    rpassword::prompt_password(format!("value for '{name}': "))
        .map_err(|e| FatalError::Config(format!("failed to read agent key value: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn set_and_delete_each_checkpoint_the_config_repo() {
        let dir = tempfile::tempdir().unwrap();

        // First write has nothing preexisting to checkpoint (nothing to
        // protect yet); the checkpoint appears from the second write on.
        run_agent_keys_command_at(
            dir.path().to_path_buf(),
            &AgentKeysCommand::Set {
                name: "first".to_string(),
                value: Some("value-one-abc123".to_string()),
                stdin: false,
                description: None,
            },
        )
        .await
        .unwrap();

        run_agent_keys_command_at(
            dir.path().to_path_buf(),
            &AgentKeysCommand::Set {
                name: "second".to_string(),
                value: Some("value-two-abc123".to_string()),
                stdin: false,
                description: None,
            },
        )
        .await
        .unwrap();

        run_agent_keys_command_at(
            dir.path().to_path_buf(),
            &AgentKeysCommand::Delete {
                name: "second".to_string(),
            },
        )
        .await
        .unwrap();

        let engine = CheckpointEngine::open_for_cli(dir.path()).unwrap();
        let page = engine
            .list_checkpoints(residuum::checkpoints::RepoKind::Config, None, None, None)
            .await
            .unwrap();
        assert_eq!(
            page.items.len(),
            2,
            "the second set and the delete should each checkpoint, the first set should not: {:?}",
            page.items.iter().map(|c| &c.summary).collect::<Vec<_>>()
        );
        assert!(
            page.items
                .iter()
                .all(|c| c.trigger == residuum::checkpoints::CheckpointTrigger::PreConfigWrite)
        );
    }
}
