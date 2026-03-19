//! Path resolution for agent-specific directories.
//!
//! Named agents live under `~/.residuum/agent_registry/<name>/`. The default
//! (unnamed) agent uses `~/.residuum/` directly, preserving backwards compatibility.

use std::path::PathBuf;

use crate::config::Config;
use crate::error::ResiduumError;

/// Base directory for all named agents: `~/.residuum/agent_registry/`.
///
/// # Errors
///
/// Returns `ResiduumError::Config` if the home directory cannot be determined.
pub fn registry_base_dir() -> Result<PathBuf, ResiduumError> {
    Ok(Config::config_dir()?.join("agent_registry"))
}

/// Config directory for a specific named agent: `~/.residuum/agent_registry/<name>/`.
///
/// # Errors
///
/// Returns `ResiduumError::Config` if the home directory cannot be determined.
pub fn agent_config_dir(name: &str) -> Result<PathBuf, ResiduumError> {
    Ok(registry_base_dir()?.join(name))
}

/// Resolve config directory for an optional agent name.
///
/// - `None` → `~/.residuum/` (default agent)
/// - `Some(name)` → `~/.residuum/agent_registry/<name>/`
///
/// # Errors
///
/// Returns `ResiduumError::Config` if the home directory cannot be determined.
pub fn resolve_config_dir(agent_name: Option<&str>) -> Result<PathBuf, ResiduumError> {
    match agent_name {
        None => Config::config_dir(),
        Some(name) => agent_config_dir(name),
    }
}

/// Resolve PID file path for an optional agent name.
///
/// PID file is always `residuum.pid` inside the resolved config directory.
///
/// # Errors
///
/// Returns `ResiduumError::Config` if the home directory cannot be determined.
pub fn resolve_pid_path(agent_name: Option<&str>) -> Result<PathBuf, ResiduumError> {
    Ok(resolve_config_dir(agent_name)?.join("residuum.pid"))
}

/// Resolve log directory for an optional agent name.
///
/// Logs directory is always `logs/` inside the resolved config directory.
///
/// # Errors
///
/// Returns `ResiduumError::Config` if the home directory cannot be determined.
pub fn resolve_log_dir(agent_name: Option<&str>) -> Result<PathBuf, ResiduumError> {
    Ok(resolve_config_dir(agent_name)?.join("logs"))
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;

    #[test]
    fn resolve_config_dir_none_returns_default() {
        let dir = resolve_config_dir(None).unwrap();
        assert!(
            dir.ends_with(".residuum"),
            "default config dir should end with .residuum: {dir:?}"
        );
    }

    #[test]
    fn resolve_config_dir_named_returns_agent_path() {
        let dir = resolve_config_dir(Some("researcher")).unwrap();
        assert!(
            dir.ends_with("agent_registry/researcher"),
            "named agent dir should be under agent_registry: {dir:?}"
        );
    }

    #[test]
    fn resolve_pid_path_none_returns_default() {
        let pid = resolve_pid_path(None).unwrap();
        assert!(
            pid.ends_with(".residuum/residuum.pid"),
            "default pid path should be in .residuum: {pid:?}"
        );
    }

    #[test]
    fn resolve_pid_path_named_returns_agent_path() {
        let pid = resolve_pid_path(Some("coder")).unwrap();
        assert!(
            pid.ends_with("agent_registry/coder/residuum.pid"),
            "named agent pid should be under agent dir: {pid:?}"
        );
    }

    #[test]
    fn resolve_log_dir_none_returns_default() {
        let logs = resolve_log_dir(None).unwrap();
        assert!(
            logs.ends_with(".residuum/logs"),
            "default log dir should be in .residuum: {logs:?}"
        );
    }

    #[test]
    fn resolve_log_dir_named_returns_agent_path() {
        let logs = resolve_log_dir(Some("researcher")).unwrap();
        assert!(
            logs.ends_with("agent_registry/researcher/logs"),
            "named agent log dir should be under agent dir: {logs:?}"
        );
    }
}
