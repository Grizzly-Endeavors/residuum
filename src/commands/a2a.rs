//! `a2a` subcommand: manage caller keys for the A2A protocol listener.

use residuum::a2a::A2aKeys;
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
    let keys = A2aKeys::new(Config::config_dir()?);

    match command {
        A2aKeysCommand::Create { name, description } => {
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
            keys.revoke(name)
                .await
                .map_err(|e| FatalError::Config(format!("couldn't revoke A2A caller key: {e}")))?;
            println!("A2A caller key '{name}' revoked");
        }
    }

    Ok(())
}
