//! Secret subcommand: manage encrypted secret storage.

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
pub(super) fn run_secret_command(command: &SecretCommand) -> Result<(), FatalError> {
    use residuum::config::SecretStore;

    let config_dir = Config::config_dir()?;

    match command {
        SecretCommand::Set { name, value } => {
            let resolved_value = if let Some(v) = value {
                v.clone()
            } else {
                // Prompt for value with masked input
                rpassword::prompt_password(format!("value for '{name}': "))
                    .map_err(|e| FatalError::Config(format!("failed to read secret value: {e}")))?
            };

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
            let mut store = SecretStore::load(&config_dir)?;
            store.delete(name, &config_dir)?;
            println!("secret '{name}' deleted");
        }
    }

    Ok(())
}
