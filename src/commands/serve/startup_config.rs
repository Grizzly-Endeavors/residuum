//! Builds the startup error for a hub whose config exists but can't load.
//!
//! Reached only when the install already has an agent (an agent is
//! discovered by scanning for `config/config.toml` — see
//! `residuum::config::discover_agents`). A fresh install with no agent at all
//! is routed straight to the setup wizard by the caller, before this is ever
//! reached. An agent's own broken config never stops `serve`: that one agent
//! starts `failed` and the rest run.

use std::path::Path;

use residuum::util::FatalError;

/// Fail with [`invalid_hub_config_error`] if the hub config under `hub_dir`
/// doesn't load and has no last-known-good copy to fall back on (the hub
/// itself retries on one and publishes a notice once it's up if it had to).
///
/// A missing timezone is not a failure when `residuum_root` has no agent:
/// the first-run file leaves it commented out, and the setup wizard is what
/// writes it. An invalid timezone name still fails here.
pub(super) fn ensure_hub_config_loads_or_has_fallback(
    residuum_root: &Path,
    hub_dir: &Path,
) -> Result<(), FatalError> {
    match residuum::config::HubConfig::load_at_for_start(hub_dir, residuum_root) {
        Ok(_) => Ok(()),
        Err(err) => {
            let had_last_known_good = residuum::gateway::has_hub_last_known_good(hub_dir);
            match residuum::gateway::load_hub_last_known_good(hub_dir) {
                Ok(_) => {
                    tracing::warn!(error = %err, "hub config failed to load; the hub will fall back to its last-known-good copy");
                    Ok(())
                }
                Err(_) => Err(invalid_hub_config_error(hub_dir, &err, had_last_known_good)),
            }
        }
    }
}

/// Build the startup error for a hub config that failed to load, naming
/// what's wrong and whether a last-known-good fallback was also tried (it
/// failed too, or this wouldn't have been reached — see the caller).
///
/// The user's files are never modified.
pub(super) fn invalid_hub_config_error(
    hub_dir: &Path,
    err: &FatalError,
    has_last_known_good: bool,
) -> FatalError {
    let detail = match err {
        FatalError::Config(msg) => msg.clone(),
        other @ (FatalError::Workspace(_) | FatalError::Gateway(_) | FatalError::Other(_)) => {
            other.to_string()
        }
    };
    let lkg_hint = if has_last_known_good {
        "\nresiduum also tried the last configuration that worked, but it failed to start too."
    } else {
        ""
    };
    FatalError::Config(format!(
        "residuum could not start because its configuration is invalid:\n  {detail}\n\n\
         Fix config.toml in {}, then start residuum again.{lkg_hint}",
        hub_dir.display(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_names_the_hub_config_and_the_detail() {
        let err = FatalError::Config("timezone is required".to_string());
        let hub_dir = Path::new("/home/x/.residuum").join("hub");
        let message = invalid_hub_config_error(&hub_dir, &err, false).to_string();
        assert!(message.contains("timezone is required"));
        assert!(message.contains(&hub_dir.display().to_string()));
        assert!(!message.contains("last configuration that worked"));
    }

    #[test]
    fn message_names_last_known_good_when_it_was_also_tried() {
        let err = FatalError::Config("boom".to_string());
        let message = invalid_hub_config_error(Path::new("/hub"), &err, true).to_string();
        assert!(message.contains("last configuration that worked"));
    }

    /// A hub directory whose live config is broken.
    fn hub_with_broken_config() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        let hub_dir = root.path().join("hub");
        std::fs::create_dir_all(&hub_dir).unwrap();
        std::fs::write(hub_dir.join("config.toml"), "not valid toml [[[").unwrap();
        root
    }

    #[test]
    fn a_hub_config_that_loads_passes() {
        let root = tempfile::tempdir().unwrap();
        let hub_dir = root.path().join("hub");
        std::fs::create_dir_all(&hub_dir).unwrap();
        std::fs::write(hub_dir.join("config.toml"), "timezone = \"UTC\"\n").unwrap();

        ensure_hub_config_loads_or_has_fallback(root.path(), &hub_dir).unwrap();
    }

    #[test]
    fn a_missing_timezone_with_no_agents_is_ready_for_the_setup_wizard() {
        let root = tempfile::tempdir().unwrap();
        let hub_dir = root.path().join("hub");
        std::fs::create_dir_all(&hub_dir).unwrap();
        std::fs::write(
            hub_dir.join("config.toml"),
            "# timezone = \"America/New_York\"\n",
        )
        .unwrap();

        ensure_hub_config_loads_or_has_fallback(root.path(), &hub_dir).unwrap();
    }

    #[test]
    fn a_broken_hub_config_with_a_saved_copy_passes() {
        let root = hub_with_broken_config();
        let hub_dir = root.path().join("hub");
        std::fs::write(
            hub_dir.join("config.last-known-good.toml"),
            "timezone = \"UTC\"\n",
        )
        .unwrap();

        ensure_hub_config_loads_or_has_fallback(root.path(), &hub_dir).unwrap();
    }

    #[test]
    fn a_broken_hub_config_without_a_saved_copy_is_reported() {
        let root = hub_with_broken_config();
        let hub_dir = root.path().join("hub");

        let message = ensure_hub_config_loads_or_has_fallback(root.path(), &hub_dir)
            .unwrap_err()
            .to_string();

        assert!(message.contains("configuration is invalid"), "{message}");
        assert!(
            !message.contains("last configuration that worked"),
            "{message}"
        );
    }
}
