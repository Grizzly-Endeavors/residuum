//! NOTIFY.yml loading.

use std::path::Path;

use crate::error::IronclawError;

use super::types::NotifyConfig;

/// Load notification routing config from a NOTIFY.yml file.
///
/// Returns a default empty config if the file does not exist.
///
/// # Errors
///
/// Returns `IronclawError::Workspace` if the file exists but cannot be read or parsed.
pub fn load_notify_config(path: &Path) -> Result<NotifyConfig, IronclawError> {
    match std::fs::read_to_string(path) {
        Ok(content) => serde_yml::from_str(&content).map_err(|e| {
            IronclawError::Workspace(format!(
                "failed to parse NOTIFY.yml at {}: {e}",
                path.display()
            ))
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(NotifyConfig::default()),
        Err(e) => Err(IronclawError::Workspace(format!(
            "failed to read NOTIFY.yml at {}: {e}",
            path.display()
        ))),
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;

    #[test]
    fn load_missing_file_returns_default() {
        let path = std::path::Path::new("/tmp/nonexistent_notify_test.yml");
        let cfg = load_notify_config(path).unwrap();
        assert!(cfg.0.is_empty(), "missing file should return empty config");
    }

    #[test]
    fn load_valid_yaml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("NOTIFY.yml");
        std::fs::write(
            &path,
            "agent_feed:\n  - email_check\n  - deploy_check\ninbox:\n  - backup\n",
        )
        .unwrap();

        let cfg = load_notify_config(&path).unwrap();
        assert_eq!(cfg.0.len(), 2, "should have two channels");
        assert_eq!(
            cfg.0.get("agent_feed").map(Vec::len),
            Some(2),
            "agent_feed should have two tasks"
        );
    }

    #[test]
    fn load_empty_yaml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("NOTIFY.yml");
        std::fs::write(&path, "agent_feed: []\ninbox: []\n").unwrap();

        let cfg = load_notify_config(&path).unwrap();
        assert_eq!(cfg.0.len(), 2, "should have two channels (empty lists)");
    }

    #[test]
    fn load_invalid_yaml_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("NOTIFY.yml");
        std::fs::write(&path, "not: [valid: yaml: {{}}").unwrap();

        let result = load_notify_config(&path);
        assert!(result.is_err(), "invalid YAML should error");
    }
}
