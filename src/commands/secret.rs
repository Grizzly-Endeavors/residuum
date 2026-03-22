//! Secret subcommand: manage encrypted secret storage.

use residuum::config::Config;
use residuum::util::FatalError;

/// Run the `secret` subcommand — manage encrypted secret storage.
///
/// Subcommands:
/// - `residuum secret set <name> [value]` — store a secret (prompts for value if omitted)
/// - `residuum secret list` — list stored secret names
/// - `residuum secret delete <name>` — remove a secret
pub(super) fn run_secret_command(args: &[String]) -> Result<(), FatalError> {
    use residuum::config::SecretStore;

    let config_dir = Config::config_dir()?;
    let sub = args.get(2).map(String::as_str);

    match sub {
        Some("set") => {
            let Some(name) = args.get(3) else {
                println!("usage: residuum secret set <name> [value]");
                return Ok(());
            };

            let value = if let Some(v) = args.get(4) {
                v.clone()
            } else {
                // Prompt for value with masked input
                rpassword::prompt_password(format!("value for '{name}': "))
                    .map_err(|e| FatalError::Config(format!("failed to read secret value: {e}")))?
            };

            let mut store = SecretStore::load(&config_dir)?;
            store.set(name, &value, &config_dir)?;
            println!("secret '{name}' saved");
        }
        Some("list") => {
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
        Some("delete") => {
            let Some(name) = args.get(3) else {
                println!("usage: residuum secret delete <name>");
                return Ok(());
            };

            let mut store = SecretStore::load(&config_dir)?;
            store.delete(name, &config_dir)?;
            println!("secret '{name}' deleted");
        }
        _ => {
            println!("usage: residuum secret <set|list|delete>");
            println!();
            println!("  set <name> [value]  store a secret (prompts if value omitted)");
            println!("  list                list stored secret names");
            println!("  delete <name>       remove a secret");
        }
    }

    Ok(())
}
