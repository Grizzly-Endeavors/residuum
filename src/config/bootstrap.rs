//! Bootstrap logic for the config directory on first run.

use std::path::{Path, PathBuf};

use crate::util::FatalError;

/// Minimal config.toml written on first run — user edits this.
const MINIMAL_CONFIG: &str = "# Residuum configuration. See config.example.toml for all options.\n\
    \n\
    # timezone = \"America/New_York\"  # REQUIRED: IANA timezone name\n";

/// Minimal providers.toml written on first run — user edits this.
const MINIMAL_PROVIDERS: &str = "# Provider and model configuration. See providers.example.toml for all options.\n\
    \n\
    [models]\n\
    main = \"anthropic/claude-sonnet-4-6\"\n";

/// Full reference config always regenerated on startup.
const EXAMPLE_CONFIG: &str = include_str!("../../assets/config.example.toml");

/// Full reference providers config always regenerated on startup.
const EXAMPLE_PROVIDERS: &str = include_str!("../../assets/providers.example.toml");

/// Get the default config directory (`~/.residuum/`).
pub(crate) fn default_config_dir() -> Result<PathBuf, FatalError> {
    dirs::home_dir()
        .map(|h| h.join(".residuum"))
        .ok_or_else(|| FatalError::Config("could not determine home directory".to_string()))
}

/// Get the default workspace directory (`~/.residuum/workspace/`).
pub(super) fn default_workspace_dir() -> Result<PathBuf, FatalError> {
    default_config_dir().map(|d| d.join("workspace"))
}

/// Write a file only if it doesn't already exist.
///
/// Returns `true` if the file was written, `false` if it already existed.
fn write_if_absent(path: &Path, content: &str) -> Result<bool, FatalError> {
    if path.exists() {
        return Ok(false);
    }
    std::fs::write(path, content).map_err(|e| {
        FatalError::Config(format!(
            "failed to write {} at {}: {e}",
            path.file_name().and_then(|n| n.to_str()).unwrap_or("file"),
            path.display()
        ))
    })?;
    Ok(true)
}

/// Write bootstrap config files to `dir`.
///
/// Creates the directory if absent, writes `config.toml` only if absent,
/// and always regenerates `config.example.toml`.
///
/// # Errors
/// Returns `FatalError::Config` if the directory or files cannot be written.
pub(super) fn bootstrap_at(dir: &Path) -> Result<(), FatalError> {
    if !dir.exists() {
        std::fs::create_dir_all(dir).map_err(|e| {
            FatalError::Config(format!(
                "failed to create config directory {}: {e}",
                dir.display()
            ))
        })?;
    }

    let config_path = dir.join("config.toml");
    if write_if_absent(&config_path, MINIMAL_CONFIG)? {
        tracing::info!(path = %config_path.display(), "wrote initial config.toml");
    }

    let providers_path = dir.join("providers.toml");
    if write_if_absent(&providers_path, MINIMAL_PROVIDERS)? {
        tracing::info!(path = %providers_path.display(), "wrote initial providers.toml");
    }

    // Always regenerate example files
    let example_path = dir.join("config.example.toml");
    std::fs::write(&example_path, EXAMPLE_CONFIG).map_err(|e| {
        FatalError::Config(format!(
            "failed to write config.example.toml at {}: {e}",
            example_path.display()
        ))
    })?;

    let providers_example_path = dir.join("providers.example.toml");
    std::fs::write(&providers_example_path, EXAMPLE_PROVIDERS).map_err(|e| {
        FatalError::Config(format!(
            "failed to write providers.example.toml at {}: {e}",
            providers_example_path.display()
        ))
    })?;

    tracing::debug!(
        config_example = %example_path.display(),
        providers_example = %providers_example_path.display(),
        "regenerated example config files"
    );

    // workspace/config/ directory with starter files
    let ws_config_dir = dir.join("workspace").join("config");
    if !ws_config_dir.exists() {
        std::fs::create_dir_all(&ws_config_dir).map_err(|e| {
            FatalError::Config(format!(
                "failed to create workspace/config at {}: {e}",
                ws_config_dir.display()
            ))
        })?;
    }

    let mcp_path = ws_config_dir.join("mcp.json");
    if write_if_absent(&mcp_path, "{ \"mcpServers\": {} }\n")? {
        tracing::info!(path = %mcp_path.display(), "wrote initial mcp.json");
    }

    let channels_path = ws_config_dir.join("channels.toml");
    if write_if_absent(
        &channels_path,
        "# Notification channel configuration. See channels.example.toml for options.\n",
    )? {
        tracing::info!(path = %channels_path.display(), "wrote initial channels.toml");
    }

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
            body.contains("timezone"),
            "config.toml should reference timezone"
        );
    }

    #[test]
    fn bootstrap_writes_minimal_providers() {
        let dir = tempdir().unwrap();
        let providers_path = dir.path().join("providers.toml");
        assert!(
            !providers_path.exists(),
            "providers.toml should not exist yet"
        );
        bootstrap_at(dir.path()).unwrap();
        assert!(providers_path.exists(), "providers.toml should be written");
        let body = std::fs::read_to_string(&providers_path).unwrap();
        assert!(
            body.contains("[models]"),
            "providers.toml should contain [models] section"
        );
        assert!(
            body.contains("main"),
            "providers.toml should contain main key"
        );
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
    fn bootstrap_skips_existing_providers() {
        let dir = tempdir().unwrap();
        let providers_path = dir.path().join("providers.toml");
        std::fs::write(&providers_path, "# user providers").unwrap();
        bootstrap_at(dir.path()).unwrap();
        let body = std::fs::read_to_string(&providers_path).unwrap();
        assert_eq!(
            body, "# user providers",
            "existing providers.toml should not be overwritten"
        );
    }

    #[test]
    fn bootstrap_always_writes_example_files() {
        let dir = tempdir().unwrap();
        let example_path = dir.path().join("config.example.toml");
        std::fs::write(&example_path, "# old content").unwrap();
        bootstrap_at(dir.path()).unwrap();
        let body = std::fs::read_to_string(&example_path).unwrap();
        assert_ne!(
            body, "# old content",
            "config.example.toml should be regenerated"
        );

        let prov_example = dir.path().join("providers.example.toml");
        assert!(
            prov_example.exists(),
            "providers.example.toml should be written"
        );
        let prov_body = std::fs::read_to_string(&prov_example).unwrap();
        assert!(
            prov_body.contains("[models]"),
            "providers example should contain [models] section"
        );
    }

    #[test]
    fn bootstrap_config_example_contains_key_sections() {
        let dir = tempdir().unwrap();
        bootstrap_at(dir.path()).unwrap();
        let body = std::fs::read_to_string(dir.path().join("config.example.toml")).unwrap();
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
            body.contains("[agent]"),
            "example should contain agent section"
        );
    }

    #[test]
    fn bootstrap_creates_workspace_config_dir() {
        let dir = tempdir().unwrap();
        bootstrap_at(dir.path()).unwrap();

        let mcp_json = dir.path().join("workspace").join("config").join("mcp.json");
        assert!(mcp_json.exists(), "mcp.json should be created");
        let mcp_body = std::fs::read_to_string(&mcp_json).unwrap();
        assert!(
            mcp_body.contains("mcpServers"),
            "mcp.json should contain mcpServers key"
        );

        let channels_toml = dir
            .path()
            .join("workspace")
            .join("config")
            .join("channels.toml");
        assert!(channels_toml.exists(), "channels.toml should be created");
    }
}
