//! Builds the startup error for an agent whose config exists but can't load.
//!
//! Reached only when an agent directory already exists (an agent is
//! discovered by scanning for `config/config.toml` — see
//! `residuum::config::discover_single_agent`), so there's no "not set up
//! yet" case to distinguish here any more: a fresh install with no agent at
//! all is routed straight to the setup wizard by the caller, before this is
//! ever reached.

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
    let hub = match residuum::config::HubConfig::load_at(&hub_dir) {
        Ok(hub) => hub,
        Err(err) if residuum::gateway::has_hub_last_known_good(&hub_dir) => {
            tracing::warn!(error = %err, "hub config failed to load; the gateway will fall back to its last-known-good copy");
            return Ok(());
        }
        Err(err) => return Err(invalid_config_error(&hub_dir, &agent_dir, &err, false)),
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
        let message = invalid_config_error(
            Path::new("/home/x/.residuum/hub"),
            Path::new("/home/x/.residuum/assistant"),
            &err,
            false,
        )
        .to_string();
        assert!(message.contains("timezone is required"));
        assert!(message.contains("/home/x/.residuum/hub"));
        assert!(message.contains("/home/x/.residuum/assistant/config"));
        assert!(!message.contains("last configuration that worked"));
    }

    #[test]
    fn message_names_last_known_good_when_it_was_also_tried() {
        let err = FatalError::Config("boom".to_string());
        let message =
            invalid_config_error(Path::new("/hub"), Path::new("/agent"), &err, true).to_string();
        assert!(message.contains("last configuration that worked"));
    }
}
