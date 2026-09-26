//! `a2a` subcommand: manage caller keys for the A2A protocol listener.

use std::path::PathBuf;

use residuum::a2a::A2aKeys;
use residuum::checkpoints::CheckpointEngine;
use residuum::config::Config;
use residuum::util::FatalError;

/// A2A subcommands.
#[derive(clap::Subcommand)]
pub(super) enum A2aCommand {
    /// Manage caller keys other agents present to reach this instance
    Keys {
        #[command(subcommand)]
        command: A2aKeysCommand,
    },
}

/// A2A caller-key management subcommands.
#[derive(clap::Subcommand)]
pub(super) enum A2aKeysCommand {
    /// Mint a caller key and print the token once
    Create {
        /// Key name: lowercase letters, digits, underscores
        name: String,
        /// What this key is for, shown in listings
        #[arg(long, short)]
        description: Option<String>,
    },
    /// List caller keys (names, descriptions, creation time — never tokens)
    List,
    /// Revoke a caller key
    Revoke {
        /// Name of the key to revoke
        name: String,
    },
}

/// Run the `a2a` subcommand.
pub(super) async fn run_a2a_command(command: &A2aCommand) -> Result<(), FatalError> {
    match command {
        A2aCommand::Keys { command } => run_a2a_keys_command(command).await,
    }
}

async fn run_a2a_keys_command(command: &A2aKeysCommand) -> Result<(), FatalError> {
    run_a2a_keys_command_at(Config::config_dir()?, command).await
}

/// [`run_a2a_keys_command`] against an explicit config directory, so the
/// dispatch logic is testable without touching the real `~/.residuum`.
async fn run_a2a_keys_command_at(
    config_dir: PathBuf,
    command: &A2aKeysCommand,
) -> Result<(), FatalError> {
    let checkpoints = CheckpointEngine::open_for_cli(&config_dir);
    let keys = A2aKeys::new(config_dir);

    match command {
        A2aKeysCommand::Create { name, description } => {
            super::checkpoint_config_before_write(
                checkpoints.as_ref(),
                format!("CLI create a2a key '{name}'"),
            )
            .await;
            let token = keys
                .create(name, description.as_deref())
                .await
                .map_err(|e| FatalError::Config(format!("couldn't create A2A caller key: {e}")))?;
            println!("A2A caller key '{name}' created.");
            println!();
            println!("  {token}");
            println!();
            println!(
                "Store this now — it is shown only once and is not recoverable. \
                 Give it to the agent that should present it as `Authorization: Bearer {token}`."
            );
        }
        A2aKeysCommand::List => {
            let snapshot = keys
                .snapshot()
                .await
                .map_err(|e| FatalError::Config(format!("couldn't read A2A caller keys: {e}")))?;
            let list = snapshot.list();
            if list.is_empty() {
                println!("no A2A caller keys stored");
            }
            for key in list {
                let description = if key.description.is_empty() {
                    String::new()
                } else {
                    format!("  {}", key.description)
                };
                println!(
                    "{}  created {}{description}",
                    key.name,
                    key.created_at.to_rfc3339()
                );
            }
        }
        A2aKeysCommand::Revoke { name } => {
            super::checkpoint_config_before_write(
                checkpoints.as_ref(),
                format!("CLI revoke a2a key '{name}'"),
            )
            .await;
            keys.revoke(name)
                .await
                .map_err(|e| FatalError::Config(format!("couldn't revoke A2A caller key: {e}")))?;
            println!("A2A caller key '{name}' revoked");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn create_list_revoke_roundtrip_succeeds() {
        let dir = tempfile::tempdir().unwrap();

        run_a2a_keys_command_at(
            dir.path().to_path_buf(),
            &A2aKeysCommand::Create {
                name: "laptop".to_string(),
                description: Some("my other instance".to_string()),
            },
        )
        .await
        .unwrap();

        run_a2a_keys_command_at(dir.path().to_path_buf(), &A2aKeysCommand::List)
            .await
            .unwrap();

        run_a2a_keys_command_at(
            dir.path().to_path_buf(),
            &A2aKeysCommand::Revoke {
                name: "laptop".to_string(),
            },
        )
        .await
        .unwrap();

        let keys = A2aKeys::new(dir.path());
        assert!(keys.snapshot().await.unwrap().list().is_empty());
    }

    #[tokio::test]
    async fn list_on_an_empty_store_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        run_a2a_keys_command_at(dir.path().to_path_buf(), &A2aKeysCommand::List)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn create_and_revoke_each_checkpoint_the_config_repo() {
        let dir = tempfile::tempdir().unwrap();

        // First write has nothing preexisting to checkpoint (nothing to
        // protect yet); the checkpoint appears from the second write on.
        run_a2a_keys_command_at(
            dir.path().to_path_buf(),
            &A2aKeysCommand::Create {
                name: "first".to_string(),
                description: None,
            },
        )
        .await
        .unwrap();

        run_a2a_keys_command_at(
            dir.path().to_path_buf(),
            &A2aKeysCommand::Create {
                name: "second".to_string(),
                description: None,
            },
        )
        .await
        .unwrap();

        run_a2a_keys_command_at(
            dir.path().to_path_buf(),
            &A2aKeysCommand::Revoke {
                name: "second".to_string(),
            },
        )
        .await
        .unwrap();

        let engine = CheckpointEngine::open_for_cli(dir.path()).unwrap();
        let page = engine
            .list_checkpoints(
                residuum::checkpoints::RepoKind::Config,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            page.items.len(),
            2,
            "the second create and the revoke should each checkpoint, the first create should not: {:?}",
            page.items.iter().map(|c| &c.summary).collect::<Vec<_>>()
        );
        assert!(
            page.items
                .iter()
                .all(|c| c.trigger == residuum::checkpoints::CheckpointTrigger::PreConfigWrite)
        );
    }

    #[tokio::test]
    async fn create_with_invalid_name_reports_the_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = run_a2a_keys_command_at(
            dir.path().to_path_buf(),
            &A2aKeysCommand::Create {
                name: "Bad Name".to_string(),
                description: None,
            },
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("couldn't create A2A caller key"));
    }

    #[tokio::test]
    async fn revoke_unknown_key_reports_the_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = run_a2a_keys_command_at(
            dir.path().to_path_buf(),
            &A2aKeysCommand::Revoke {
                name: "nope".to_string(),
            },
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("couldn't revoke A2A caller key"));
    }
}
