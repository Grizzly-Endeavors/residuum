//! Bootstrap logic for the hub directory and an agent's config directory on
//! first run.

use std::path::Path;

use crate::util::FatalError;

/// Minimal hub `config.toml` written on first run — user edits this.
const MINIMAL_HUB_CONFIG: &str = "# Hub configuration. See hub-config.example.toml for all options.\n\
    \n\
    # timezone = \"America/New_York\"  # REQUIRED: IANA timezone name\n";

/// Minimal agent `config.toml` written on first run — user edits this.
const MINIMAL_AGENT_CONFIG: &str =
    "# Agent configuration. See config.example.toml for all options.\n";

/// Minimal `providers.toml` written on first run — user edits this.
const MINIMAL_PROVIDERS: &str = "# Provider and model configuration. See providers.example.toml for all options.\n\
    \n\
    [models]\n\
    main = \"anthropic/claude-sonnet-4-6\"\n";

/// Full reference hub config always regenerated on startup.
const EXAMPLE_HUB_CONFIG: &str = include_str!("../../assets/hub-config.example.toml");

/// Full reference agent config always regenerated on startup.
const EXAMPLE_CONFIG: &str = include_str!("../../assets/config.example.toml");

/// Full reference providers config always regenerated on startup.
const EXAMPLE_PROVIDERS: &str = include_str!("../../assets/providers.example.toml");

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

fn ensure_dir(dir: &Path, label: &str) -> Result<(), FatalError> {
    if dir.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(dir).map_err(|e| {
        FatalError::Config(format!(
            "failed to create {label} directory {}: {e}",
            dir.display()
        ))
    })
}

/// Write bootstrap files to `hub_dir` (`~/.residuum/hub`).
///
/// Creates the directory if absent, writes `config.toml` only if absent,
/// and always regenerates `config.example.toml`. Also ensures `bin/` and
/// `logs/` exist; `checkpoints/` is created lazily by `CheckpointEngine::new`.
///
/// # Errors
/// Returns `FatalError::Config` if the directory or files cannot be written.
pub(super) fn bootstrap_hub_at(hub_dir: &Path) -> Result<(), FatalError> {
    ensure_dir(hub_dir, "hub")?;

    let config_path = hub_dir.join("config.toml");
    if write_if_absent(&config_path, MINIMAL_HUB_CONFIG)? {
        tracing::info!(path = %config_path.display(), "wrote initial hub config.toml");
    }

    let example_path = hub_dir.join("config.example.toml");
    std::fs::write(&example_path, EXAMPLE_HUB_CONFIG).map_err(|e| {
        FatalError::Config(format!(
            "failed to write config.example.toml at {}: {e}",
            example_path.display()
        ))
    })?;

    // Default persistent tools dir (~/.residuum/hub/bin), shared by every
    // agent the hub hosts. Drop static binaries here to make them
    // resolvable by spawned children without rebuilding the image.
    let hub_paths = super::HubPaths::new(hub_dir);
    ensure_dir(&hub_paths.bin_dir(), "hub tools")?;
    ensure_dir(&hub_paths.logs_dir(), "hub logs")?;

    tracing::debug!(hub_dir = %hub_dir.display(), "hub directory bootstrapped");
    Ok(())
}

/// Write bootstrap files to an agent's `config/` directory
/// (`~/.residuum/<agent-name>/config`).
///
/// Creates the directory if absent, writes `config.toml`/`providers.toml`
/// only if absent, and always regenerates the `.example.toml` templates.
///
/// # Errors
/// Returns `FatalError::Config` if the directory or files cannot be written.
pub(super) fn bootstrap_agent_at(agent_config_dir: &Path) -> Result<(), FatalError> {
    ensure_dir(agent_config_dir, "agent config")?;

    let config_path = agent_config_dir.join("config.toml");
    if write_if_absent(&config_path, MINIMAL_AGENT_CONFIG)? {
        tracing::info!(path = %config_path.display(), "wrote initial agent config.toml");
    }

    let providers_path = agent_config_dir.join("providers.toml");
    if write_if_absent(&providers_path, MINIMAL_PROVIDERS)? {
        tracing::info!(path = %providers_path.display(), "wrote initial providers.toml");
    }

    let example_path = agent_config_dir.join("config.example.toml");
    std::fs::write(&example_path, EXAMPLE_CONFIG).map_err(|e| {
        FatalError::Config(format!(
            "failed to write config.example.toml at {}: {e}",
            example_path.display()
        ))
    })?;

    let providers_example_path = agent_config_dir.join("providers.example.toml");
    std::fs::write(&providers_example_path, EXAMPLE_PROVIDERS).map_err(|e| {
        FatalError::Config(format!(
            "failed to write providers.example.toml at {}: {e}",
            providers_example_path.display()
        ))
    })?;

    tracing::debug!(
        agent_config_dir = %agent_config_dir.display(),
        "agent config directory bootstrapped"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn bootstrap_hub_creates_dir_and_minimal_config() {
        let base = tempdir().unwrap();
        let dir = base.path().join("hub");
        assert!(!dir.exists());
        bootstrap_hub_at(&dir).unwrap();
        assert!(dir.exists());
        let body = std::fs::read_to_string(dir.join("config.toml")).unwrap();
        assert!(body.contains("timezone"));
        assert!(dir.join("bin").is_dir());
        assert!(dir.join("logs").is_dir());
    }

    #[test]
    fn bootstrap_hub_skips_existing_config() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(&config_path, "# user customization").unwrap();
        bootstrap_hub_at(dir.path()).unwrap();
        assert_eq!(
            std::fs::read_to_string(&config_path).unwrap(),
            "# user customization"
        );
    }

    #[test]
    fn bootstrap_hub_always_regenerates_example() {
        let dir = tempdir().unwrap();
        let example_path = dir.path().join("config.example.toml");
        std::fs::write(&example_path, "# old content").unwrap();
        bootstrap_hub_at(dir.path()).unwrap();
        let body = std::fs::read_to_string(&example_path).unwrap();
        assert_ne!(body, "# old content");
        assert!(body.contains("[gateway]"));
    }

    #[test]
    fn bootstrap_agent_creates_config_and_providers() {
        let base = tempdir().unwrap();
        let dir = base.path().join("sam/config");
        assert!(!dir.exists());
        bootstrap_agent_at(&dir).unwrap();
        assert!(dir.join("config.toml").exists());
        let providers = std::fs::read_to_string(dir.join("providers.toml")).unwrap();
        assert!(providers.contains("[models]"));
        assert!(providers.contains("main"));
    }

    #[test]
    fn bootstrap_agent_skips_existing_files() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "# user customization").unwrap();
        std::fs::write(dir.path().join("providers.toml"), "# user providers").unwrap();
        bootstrap_agent_at(dir.path()).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("config.toml")).unwrap(),
            "# user customization"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("providers.toml")).unwrap(),
            "# user providers"
        );
    }

    #[test]
    fn bootstrap_agent_always_regenerates_examples() {
        let dir = tempdir().unwrap();
        let example_path = dir.path().join("config.example.toml");
        std::fs::write(&example_path, "# old content").unwrap();
        bootstrap_agent_at(dir.path()).unwrap();
        let body = std::fs::read_to_string(&example_path).unwrap();
        assert_ne!(body, "# old content");
        assert!(body.contains("[memory]"));
        assert!(body.contains("[agent]"));

        let prov_example = dir.path().join("providers.example.toml");
        assert!(prov_example.exists());
        let prov_body = std::fs::read_to_string(&prov_example).unwrap();
        assert!(prov_body.contains("[models]"));
    }

    #[test]
    fn bootstrap_agent_config_example_does_not_contain_hub_only_sections() {
        let dir = tempdir().unwrap();
        bootstrap_agent_at(dir.path()).unwrap();
        let body = std::fs::read_to_string(dir.path().join("config.example.toml")).unwrap();
        assert!(
            !body.contains("[gateway]"),
            "gateway is hub-only, must not appear in the agent example"
        );
        assert!(
            !body.contains("[cloud]"),
            "cloud is hub-only, must not appear in the agent example"
        );
    }

    #[test]
    fn bootstrap_hub_config_example_does_not_contain_agent_only_sections() {
        let dir = tempdir().unwrap();
        bootstrap_hub_at(dir.path()).unwrap();
        let body = std::fs::read_to_string(dir.path().join("config.example.toml")).unwrap();
        assert!(
            !body.contains("[memory]"),
            "memory is agent-only, must not appear in the hub example"
        );
        assert!(
            !body.contains("[discord]"),
            "discord is agent-only, must not appear in the hub example"
        );
    }
}
