//! Builds the startup error for an agent whose config exists but can't load.
//!
//! Reached only when an agent directory already exists (an agent is
//! discovered by scanning for `config/config.toml` — see
//! `residuum::config::discover_single_agent`). A fresh install with no agent
//! at all is routed straight to the setup wizard by the caller, before this
//! is ever reached.

use std::path::Path;

use residuum::util::FatalError;

/// Fail with [`invalid_config_error`] if the hub or agent config under
/// `residuum_root` doesn't load and neither has a last-known-good copy to
/// fall back on (the gateway itself retries on one and publishes a notice
/// once it's up if it had to).
pub(super) fn ensure_config_loads_or_has_fallback(
    residuum_root: &Path,
    agent_name: &str,
) -> Result<(), FatalError> {
    let hub_dir = residuum::config::paths::hub_dir(residuum_root);
    let agent_dir = residuum::config::paths::agent_dir(residuum_root, agent_name);
    // A broken hub config falls back to its last-known-good copy, and the
    // agent config is then validated against that copy, so a bad agent
    // config gets the same friendly error (or its own fallback) either way.
    let hub = match residuum::config::HubConfig::load_at(&hub_dir) {
        Ok(hub) => hub,
        Err(err) => {
            let had_last_known_good = residuum::gateway::has_hub_last_known_good(&hub_dir);
            match residuum::gateway::load_hub_last_known_good(&hub_dir) {
                Ok(hub) => {
                    tracing::warn!(error = %err, "hub config failed to load; the gateway will fall back to its last-known-good copy");
                    hub
                }
                Err(_) => {
                    return Err(invalid_config_error(
                        &hub_dir,
                        &agent_dir,
                        &err,
                        had_last_known_good,
                    ));
                }
            }
        }
    };
    match residuum::config::Config::load_agent_at(&agent_dir, &hub) {
        Ok(_) => Ok(()),
        Err(err) if residuum::gateway::has_last_known_good(&agent_dir.join("config")) => {
            tracing::warn!(error = %err, "agent config failed to load; the gateway will fall back to its last-known-good copy");
            Ok(())
        }
        Err(err) => Err(invalid_config_error(&hub_dir, &agent_dir, &err, false)),
    }
}

/// Build the startup error for a hub/agent config pair that failed to load,
/// naming what's wrong and whether a last-known-good fallback was also
/// tried (it failed too, or this wouldn't have been reached — see the
/// caller).
///
/// The user's files are never modified.
pub(super) fn invalid_config_error(
    hub_dir: &Path,
    agent_dir: &Path,
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
         Fix hub/config.toml in {}, or config.toml/providers.toml in {}, then start residuum \
         again.{lkg_hint}",
        hub_dir.display(),
        agent_dir.join("config").display(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_names_both_possible_files_and_the_detail() {
        let err = FatalError::Config("timezone is required".to_string());
        let root = Path::new("/home/x/.residuum");
        let hub_dir = root.join("hub");
        let agent_dir = root.join("assistant");
        let message = invalid_config_error(&hub_dir, &agent_dir, &err, false).to_string();
        assert!(message.contains("timezone is required"));
        assert!(message.contains(&hub_dir.display().to_string()));
        let agent_config_dir = agent_dir.join("config");
        assert!(message.contains(&agent_config_dir.display().to_string()));
        assert!(!message.contains("last configuration that worked"));
    }

    #[test]
    fn message_names_last_known_good_when_it_was_also_tried() {
        let err = FatalError::Config("boom".to_string());
        let message =
            invalid_config_error(Path::new("/hub"), Path::new("/agent"), &err, true).to_string();
        assert!(message.contains("last configuration that worked"));
    }

    const VALID_PROVIDERS: &str = "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n";

    /// A residuum root with a hub dir and an agent `scout` whose
    /// `config.toml` is the given text.
    fn root_with_agent(agent_config: &str) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        let hub_dir = root.path().join("hub");
        let config_dir = root.path().join("scout").join("config");
        std::fs::create_dir_all(&hub_dir).unwrap();
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(hub_dir.join("config.toml"), "timezone = \"UTC\"\n").unwrap();
        std::fs::write(config_dir.join("config.toml"), agent_config).unwrap();
        std::fs::write(config_dir.join("providers.toml"), VALID_PROVIDERS).unwrap();
        root
    }

    /// Break the live hub config but leave a good last-known-good copy.
    fn break_hub_with_good_copy(root: &Path) {
        let hub_dir = root.join("hub");
        std::fs::write(
            hub_dir.join("config.last-known-good.toml"),
            "timezone = \"UTC\"\n",
        )
        .unwrap();
        std::fs::write(hub_dir.join("config.toml"), "not valid toml [[[").unwrap();
    }

    #[test]
    fn hub_fallback_with_a_good_agent_config_passes() {
        let root = root_with_agent("");
        break_hub_with_good_copy(root.path());
        ensure_config_loads_or_has_fallback(root.path(), "scout").unwrap();
    }

    #[test]
    fn hub_fallback_still_reports_a_bad_agent_config_in_plain_language() {
        let root = root_with_agent("not valid toml [[[");
        break_hub_with_good_copy(root.path());

        let message = ensure_config_loads_or_has_fallback(root.path(), "scout")
            .unwrap_err()
            .to_string();

        assert!(message.contains("configuration is invalid"), "{message}");
        assert!(message.contains("scout"), "{message}");
    }

    #[test]
    fn hub_fallback_with_a_bad_agent_config_uses_the_agents_own_fallback() {
        let root = root_with_agent("not valid toml [[[");
        break_hub_with_good_copy(root.path());
        let hub_dir = root.path().join("hub");
        std::fs::write(hub_dir.join("scout.config.last-known-good.toml"), "").unwrap();
        std::fs::write(
            hub_dir.join("scout.providers.last-known-good.toml"),
            VALID_PROVIDERS,
        )
        .unwrap();

        ensure_config_loads_or_has_fallback(root.path(), "scout").unwrap();
    }

    #[test]
    fn a_broken_hub_config_without_a_saved_copy_is_reported() {
        let root = root_with_agent("");
        std::fs::write(
            root.path().join("hub").join("config.toml"),
            "not valid toml [[[",
        )
        .unwrap();

        let message = ensure_config_loads_or_has_fallback(root.path(), "scout")
            .unwrap_err()
            .to_string();

        assert!(message.contains("configuration is invalid"), "{message}");
        assert!(
            !message.contains("last configuration that worked"),
            "{message}"
        );
    }
}
