//! Bootstrap logic for the config directory on first run.

use std::path::{Path, PathBuf};

use crate::error::IronclawError;

/// Minimal config.toml written on first run — user edits this.
const MINIMAL_CONFIG: &str = "# IronClaw configuration. See config.example.toml for all options.\n\
    \n\
    # timezone = \"America/New_York\"  # REQUIRED: IANA timezone name\n\
    \n\
    [models]\n\
    main = \"anthropic/claude-sonnet-4-6\"\n";

/// Full reference config always regenerated on startup.
///
/// Every option is shown with its default and a brief comment.
const EXAMPLE_CONFIG: &str = include_str!("../../assets/config.example.toml");

/// Get the default config directory (`~/.ironclaw/`).
pub(super) fn default_config_dir() -> Result<PathBuf, IronclawError> {
    dirs::home_dir()
        .map(|h| h.join(".ironclaw"))
        .ok_or_else(|| IronclawError::Config("could not determine home directory".to_string()))
}

/// Get the default workspace directory (`~/.ironclaw/workspace/`).
pub(super) fn default_workspace_dir() -> Result<PathBuf, IronclawError> {
    default_config_dir().map(|d| d.join("workspace"))
}

/// Write bootstrap config files to `dir`.
///
/// Creates the directory if absent, writes `config.toml` only if absent,
/// and always regenerates `config.example.toml`.
///
/// # Errors
/// Returns `IronclawError::Config` if the directory or files cannot be written.
pub(super) fn bootstrap_at(dir: &Path) -> Result<(), IronclawError> {
    if !dir.exists() {
        std::fs::create_dir_all(dir).map_err(|e| {
            IronclawError::Config(format!(
                "failed to create config directory {}: {e}",
                dir.display()
            ))
        })?;
    }

    let config_path = dir.join("config.toml");
    if !config_path.exists() {
        std::fs::write(&config_path, MINIMAL_CONFIG).map_err(|e| {
            IronclawError::Config(format!(
                "failed to write config.toml at {}: {e}",
                config_path.display()
            ))
        })?;
        tracing::info!(path = %config_path.display(), "wrote initial config.toml");
    }

    let example_path = dir.join("config.example.toml");
    std::fs::write(&example_path, EXAMPLE_CONFIG).map_err(|e| {
        IronclawError::Config(format!(
            "failed to write config.example.toml at {}: {e}",
            example_path.display()
        ))
    })?;

    Ok(())
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn bootstrap_creates_config_dir() {
        let base = tempdir().unwrap();
        let dir = base.path().join("newdir");
        assert!(!dir.exists(), "dir should not exist before bootstrap");
        bootstrap_at(&dir).unwrap();
        assert!(dir.exists(), "dir should be created by bootstrap");
    }

    #[test]
    fn bootstrap_writes_minimal_config() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        assert!(!config_path.exists(), "config.toml should not exist yet");
        bootstrap_at(dir.path()).unwrap();
        assert!(config_path.exists(), "config.toml should be written");
        let body = std::fs::read_to_string(&config_path).unwrap();
        assert!(
            body.contains("[models]"),
            "config.toml should contain [models] section"
        );
        assert!(body.contains("main"), "config.toml should contain main key");
    }

    #[test]
    fn bootstrap_skips_existing_config() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(&config_path, "# user customization").unwrap();
        bootstrap_at(dir.path()).unwrap();
        let body = std::fs::read_to_string(&config_path).unwrap();
        assert_eq!(
            body, "# user customization",
            "existing config.toml should not be overwritten"
        );
    }

    #[test]
    fn bootstrap_always_writes_example() {
        let dir = tempdir().unwrap();
        let example_path = dir.path().join("config.example.toml");
        std::fs::write(&example_path, "# old content").unwrap();
        bootstrap_at(dir.path()).unwrap();
        let body = std::fs::read_to_string(&example_path).unwrap();
        assert_ne!(
            body, "# old content",
            "config.example.toml should be regenerated"
        );
        assert!(
            body.contains("[models]"),
            "example should contain [models] section"
        );
    }

    #[test]
    fn bootstrap_example_contains_all_sections() {
        let dir = tempdir().unwrap();
        bootstrap_at(dir.path()).unwrap();
        let body = std::fs::read_to_string(dir.path().join("config.example.toml")).unwrap();
        assert!(
            body.contains("[providers]") || body.contains("providers"),
            "example should document providers"
        );
        assert!(
            body.contains("[models]"),
            "example should contain models section"
        );
        assert!(
            body.contains("[memory]"),
            "example should contain memory section"
        );
        assert!(
            body.contains("[pulse]"),
            "example should contain pulse section"
        );
        assert!(
            body.contains("[gateway]"),
            "example should contain gateway section"
        );
        assert!(
            body.contains("[discord]"),
            "example should document discord section"
        );
        assert!(
            body.contains("[webhook]"),
            "example should document webhook section"
        );
        assert!(
            body.contains("[skills]"),
            "example should document skills section"
        );
        assert!(body.contains("mcp"), "example should document mcp section");
    }
}
