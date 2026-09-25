//! Secret subcommand: manage encrypted secret storage.

use std::path::PathBuf;

use residuum::checkpoints::CheckpointEngine;
use residuum::config::Config;
use residuum::util::FatalError;

/// Secret management subcommands.
#[derive(clap::Subcommand)]
pub(super) enum SecretCommand {
    /// Store a secret (prompts for value if omitted)
    Set {
        /// Name of the secret
        name: String,
        /// Value to store (prompted interactively if omitted)
        value: Option<String>,
    },
    /// List stored secret names
    List,
    /// Remove a secret
    Delete {
        /// Name of the secret to remove
        name: String,
    },
}

/// Run the `secret` subcommand — manage encrypted secret storage.
pub(super) async fn run_secret_command(command: &SecretCommand) -> Result<(), FatalError> {
    run_secret_command_at(Config::config_dir()?, command).await
}

/// [`run_secret_command`] against an explicit config directory, so the
/// dispatch logic is testable without touching the real `~/.residuum`.
async fn run_secret_command_at(
    config_dir: PathBuf,
    command: &SecretCommand,
) -> Result<(), FatalError> {
    use residuum::config::SecretStore;

    let checkpoints = CheckpointEngine::open_for_cli(&config_dir);

    match command {
        SecretCommand::Set { name, value } => {
            let resolved_value = if let Some(v) = value {
                v.clone()
            } else {
                // Prompt for value with masked input
                rpassword::prompt_password(format!("value for '{name}': "))
                    .map_err(|e| FatalError::Config(format!("failed to read secret value: {e}")))?
            };

            super::checkpoint_config_before_write(
                checkpoints.as_ref(),
                format!("CLI set secret '{name}'"),
            )
            .await;
            let mut store = SecretStore::load(&config_dir)?;
            store.set(name, &resolved_value, &config_dir)?;
            println!("secret '{name}' saved");
        }
        SecretCommand::List => {
            let store = SecretStore::load(&config_dir)?;
            let names = store.names();
            if names.is_empty() {
                println!("no secrets stored");
            } else {
                for name in &names {
                    println!("{name}");
                }
            }
        }
        SecretCommand::Delete { name } => {
            super::checkpoint_config_before_write(
                checkpoints.as_ref(),
                format!("CLI delete secret '{name}'"),
            )
            .await;
            let mut store = SecretStore::load(&config_dir)?;
            store.delete(name, &config_dir)?;
            println!("secret '{name}' deleted");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn set_and_delete_each_checkpoint_the_config_repo() {
        let dir = tempfile::tempdir().unwrap();

        // First write has nothing preexisting to checkpoint (nothing to
        // protect yet); the checkpoint appears from the second write on.
        run_secret_command_at(
            dir.path().to_path_buf(),
            &SecretCommand::Set {
                name: "first".to_string(),
                value: Some("value-one".to_string()),
            },
        )
        .await
        .unwrap();

        run_secret_command_at(
            dir.path().to_path_buf(),
            &SecretCommand::Set {
                name: "second".to_string(),
                value: Some("value-two".to_string()),
            },
        )
        .await
        .unwrap();

        run_secret_command_at(
            dir.path().to_path_buf(),
            &SecretCommand::Delete {
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

    #[tokio::test]
    async fn list_on_an_empty_store_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        run_secret_command_at(dir.path().to_path_buf(), &SecretCommand::List)
            .await
            .unwrap();
    }
}
