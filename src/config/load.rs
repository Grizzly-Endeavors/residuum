//! Config loading and validation methods.

use std::path::PathBuf;

use crate::error::ResiduumError;

use super::Config;
use super::{bootstrap, deserialize, resolve};

impl Config {
    /// Write default config files to `~/.residuum/` if not already present.
    ///
    /// - `config.toml` is created only if absent (minimal template for the user to edit).
    /// - `config.example.toml` is always regenerated (kept in sync with the current schema).
    ///
    /// # Errors
    /// Returns `ResiduumError::Config` if the config directory or files cannot be written.
    pub fn bootstrap_config_dir() -> Result<(), ResiduumError> {
        let dir = bootstrap::default_config_dir()?;
        bootstrap::bootstrap_at(&dir)
    }

    /// Write default config files to an arbitrary directory.
    ///
    /// Same as [`bootstrap_config_dir`](Self::bootstrap_config_dir) but targets
    /// a caller-specified path instead of `~/.residuum/`.
    ///
    /// # Errors
    /// Returns `ResiduumError::Config` if the directory or files cannot be written.
    pub fn bootstrap_at_dir(dir: &std::path::Path) -> Result<(), ResiduumError> {
        bootstrap::bootstrap_at(dir)
    }

    /// Get the default config directory path (`~/.residuum/`).
    ///
    /// # Errors
    /// Returns `ResiduumError::Config` if the home directory cannot be determined.
    pub fn config_dir() -> Result<PathBuf, ResiduumError> {
        bootstrap::default_config_dir()
    }

    /// Load configuration from the default config file and environment.
    ///
    /// Priority: env vars > config file > defaults.
    ///
    /// # Errors
    /// Returns `ResiduumError::Config` if the config file exists but cannot be
    /// read or parsed, or if required values are missing.
    pub fn load() -> Result<Self, ResiduumError> {
        let config_dir = bootstrap::default_config_dir()?;
        let mut cfg = Self::load_at(&config_dir)?;
        cfg.config_dir = config_dir;
        Ok(cfg)
    }

    /// Load configuration from a specific directory.
    ///
    /// Same as [`load`](Self::load) but reads `config.toml` from the given
    /// directory instead of the default `~/.residuum/`.
    ///
    /// # Errors
    /// Returns `ResiduumError::Config` if the config file exists but cannot be
    /// read or parsed, or if required values are missing.
    pub fn load_at(config_dir: &std::path::Path) -> Result<Self, ResiduumError> {
        let config_path = config_dir.join("config.toml");
        let providers_path = config_dir.join("providers.toml");

        let file_config = if config_path.exists() {
            let contents = std::fs::read_to_string(&config_path).map_err(|e| {
                ResiduumError::Config(format!(
                    "failed to read config at {}: {e}",
                    config_path.display()
                ))
            })?;
            Some(
                toml::from_str::<deserialize::ConfigFile>(&contents).map_err(|e| {
                    ResiduumError::Config(format!(
                        "failed to parse config at {}: {e}",
                        config_path.display()
                    ))
                })?,
            )
        } else {
            None
        };

        let providers_config = if providers_path.exists() {
            let contents = std::fs::read_to_string(&providers_path).map_err(|e| {
                ResiduumError::Config(format!(
                    "failed to read providers config at {}: {e}",
                    providers_path.display()
                ))
            })?;
            Some(
                toml::from_str::<deserialize::ProvidersFile>(&contents).map_err(|e| {
                    ResiduumError::Config(format!(
                        "failed to parse providers config at {}: {e}",
                        providers_path.display()
                    ))
                })?,
            )
        } else {
            return Err(ResiduumError::Config(format!(
                "providers.toml not found at {}; run 'residuum setup' to create it",
                providers_path.display()
            )));
        };

        let mut cfg = resolve::from_file_and_env(
            file_config.as_ref(),
            providers_config.as_ref(),
            config_dir,
        )?;
        cfg.config_dir = config_dir.to_path_buf();
        Ok(cfg)
    }

    /// Validate a TOML string as a config file without saving it.
    ///
    /// Parses the TOML into the raw config structure, then runs full resolution
    /// to catch semantic errors (missing timezone, etc.). Reads the existing
    /// `providers.toml` from the config directory for model resolution.
    ///
    /// # Errors
    /// Returns a human-readable error string if validation fails.
    pub fn validate_toml(contents: &str, config_dir: &std::path::Path) -> Result<(), String> {
        let file = toml::from_str::<deserialize::ConfigFile>(contents)
            .map_err(|e| format!("TOML parse error: {e}"))?;

        // Load providers.toml from disk for resolution (may not exist during setup)
        let providers_path = config_dir.join("providers.toml");
        let providers_file = if providers_path.exists() {
            let prov_contents = std::fs::read_to_string(&providers_path)
                .map_err(|e| format!("failed to read providers.toml: {e}"))?;
            Some(
                toml::from_str::<deserialize::ProvidersFile>(&prov_contents)
                    .map_err(|e| format!("providers.toml parse error: {e}"))?,
            )
        } else {
            None
        };

        resolve::from_file_and_env(Some(&file), providers_file.as_ref(), config_dir)
            .map_err(|e| format!("{e}"))?;
        Ok(())
    }

    /// Validate a TOML string as a providers file without saving it.
    ///
    /// Parses the TOML and runs model resolution against the existing
    /// `config.toml` on disk to catch semantic errors.
    ///
    /// # Errors
    /// Returns a human-readable error string if validation fails.
    pub fn validate_providers_toml(
        contents: &str,
        config_dir: &std::path::Path,
    ) -> Result<(), String> {
        let providers_file = toml::from_str::<deserialize::ProvidersFile>(contents)
            .map_err(|e| format!("TOML parse error: {e}"))?;

        // Load config.toml from disk for resolution
        let config_path = config_dir.join("config.toml");
        let config_file = if config_path.exists() {
            let cfg_contents = std::fs::read_to_string(&config_path)
                .map_err(|e| format!("failed to read config.toml: {e}"))?;
            Some(
                toml::from_str::<deserialize::ConfigFile>(&cfg_contents)
                    .map_err(|e| format!("config.toml parse error: {e}"))?,
            )
        } else {
            None
        };

        resolve::from_file_and_env(config_file.as_ref(), Some(&providers_file), config_dir)
            .map_err(|e| format!("{e}"))?;
        Ok(())
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]
mod tests {
    use super::*;

    /// Valid minimal config TOML (no providers/models — those are in providers.toml).
    const VALID_CONFIG: &str = "timezone = \"UTC\"\n";

    /// Valid minimal providers TOML.
    const VALID_PROVIDERS: &str = "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n";

    /// Write a `providers.toml` to disk for tests that call `validate_toml`/`load_at`.
    fn write_providers(dir: &std::path::Path) {
        std::fs::write(dir.join("providers.toml"), VALID_PROVIDERS).unwrap();
    }

    #[test]
    fn validate_toml_accepts_valid_config() {
        let dir = tempfile::tempdir().unwrap();
        write_providers(dir.path());
        let result = Config::validate_toml(VALID_CONFIG, dir.path());
        assert!(result.is_ok(), "valid config should pass: {result:?}");
    }

    #[test]
    fn validate_toml_rejects_missing_timezone() {
        let dir = tempfile::tempdir().unwrap();
        write_providers(dir.path());
        // Empty config — no timezone
        let result = Config::validate_toml("", dir.path());
        assert!(result.is_err(), "missing timezone should fail validation");
        let err = result.unwrap_err();
        assert!(
            err.contains("timezone"),
            "error should mention timezone: {err}"
        );
    }

    #[test]
    fn validate_toml_resolves_secrets_from_real_store() {
        let dir = tempfile::tempdir().unwrap();
        // Store a secret
        let mut store = super::super::secrets::SecretStore::load(dir.path()).unwrap();
        store
            .set("test_api_key", "sk-test-123", dir.path())
            .unwrap();

        // Providers file that references the secret
        let providers_with_secret = r#"
[providers.my-provider]
type = "anthropic"
api_key = "secret:test_api_key"

[models]
main = "my-provider/claude-sonnet-4-6"
"#;
        std::fs::write(dir.path().join("providers.toml"), providers_with_secret).unwrap();

        let result = Config::validate_toml(VALID_CONFIG, dir.path());
        assert!(
            result.is_ok(),
            "secret reference should resolve with real store: {result:?}"
        );
    }

    #[test]
    fn load_at_returns_error_on_invalid_toml() {
        let dir = tempfile::tempdir().unwrap();
        write_providers(dir.path());
        std::fs::write(dir.path().join("config.toml"), "invalid toml syntax = [").unwrap();

        let result = Config::load_at(dir.path());
        assert!(
            result.is_err(),
            "load_at should fail on invalid TOML syntax"
        );
        let err = result.unwrap_err();
        assert!(
            matches!(err, ResiduumError::Config(_)),
            "error should be of type ResiduumError::Config, got: {err:?}"
        );
    }

    #[test]
    fn load_at_returns_error_when_providers_missing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), VALID_CONFIG).unwrap();
        // No providers.toml written

        let result = Config::load_at(dir.path());
        assert!(
            result.is_err(),
            "load_at should fail when providers.toml is missing"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("providers.toml"),
            "error should mention providers.toml: {err}"
        );
    }

    #[test]
    fn validate_toml_rejects_invalid_toml_syntax() {
        let dir = tempfile::tempdir().unwrap();
        let bad_toml = "this is not valid toml";
        let result = Config::validate_toml(bad_toml, dir.path());
        assert!(result.is_err(), "invalid TOML syntax should fail parse");
        let err = result.unwrap_err();
        assert!(
            err.contains("TOML parse error"),
            "error should mention TOML parse error: {err}"
        );
    }

    #[test]
    fn validate_providers_toml_rejects_invalid_model_format() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), VALID_CONFIG).unwrap();

        let bad_providers = r#"
[models]
main = "invalid-format"
"#;
        let result = Config::validate_providers_toml(bad_providers, dir.path());
        assert!(
            result.is_err(),
            "missing slash in model should fail validation"
        );
        let err = result.unwrap_err();
        assert!(
            err.contains("expected 'provider/model' format"),
            "error should mention expected format: {err}"
        );
    }

    #[test]
    fn idle_config_defaults_when_section_missing() {
        let dir = tempfile::tempdir().unwrap();
        write_providers(dir.path());
        std::fs::write(dir.path().join("config.toml"), VALID_CONFIG).unwrap();
        let cfg = Config::load_at(dir.path()).unwrap();
        assert_eq!(cfg.idle.timeout, std::time::Duration::from_secs(30 * 60));
        assert!(cfg.idle.idle_channel.is_none());
    }

    #[test]
    fn idle_config_timeout_zero_disables() {
        let dir = tempfile::tempdir().unwrap();
        write_providers(dir.path());
        let toml = "timezone = \"UTC\"\n\n[idle]\ntimeout_minutes = 0\n";
        std::fs::write(dir.path().join("config.toml"), toml).unwrap();
        let cfg = Config::load_at(dir.path()).unwrap();
        assert_eq!(cfg.idle.timeout, std::time::Duration::ZERO);
    }

    #[test]
    fn idle_config_explicit_values() {
        let dir = tempfile::tempdir().unwrap();
        write_providers(dir.path());
        let toml = "timezone = \"UTC\"\n\n[telegram]\ntoken = \"test-token\"\n\n[idle]\ntimeout_minutes = 15\nidle_channel = \"telegram\"\n";
        std::fs::write(dir.path().join("config.toml"), toml).unwrap();
        let cfg = Config::load_at(dir.path()).unwrap();
        assert_eq!(cfg.idle.timeout, std::time::Duration::from_secs(15 * 60));
        assert_eq!(cfg.idle.idle_channel.as_deref(), Some("telegram"));
    }

    #[test]
    fn idle_channel_rejected_for_unconfigured_interface() {
        let dir = tempfile::tempdir().unwrap();
        write_providers(dir.path());
        let toml = "timezone = \"UTC\"\n\n[idle]\nidle_channel = \"telegram\"\n";
        std::fs::write(dir.path().join("config.toml"), toml).unwrap();
        let result = Config::load_at(dir.path());
        assert!(
            result.is_err(),
            "idle_channel=telegram without [telegram] should fail"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("idle_channel") && err.contains("missing"),
            "error should mention idle_channel and missing section: {err}"
        );
    }

    #[test]
    fn idle_channel_rejected_for_unknown_name() {
        let dir = tempfile::tempdir().unwrap();
        write_providers(dir.path());
        let toml = "timezone = \"UTC\"\n\n[idle]\nidle_channel = \"sms\"\n";
        std::fs::write(dir.path().join("config.toml"), toml).unwrap();
        let result = Config::load_at(dir.path());
        assert!(
            result.is_err(),
            "idle_channel=sms should be rejected as unknown"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("not a recognized interface"),
            "error should mention unrecognized interface: {err}"
        );
    }

    #[test]
    fn idle_channel_websocket_always_valid() {
        let dir = tempfile::tempdir().unwrap();
        write_providers(dir.path());
        let toml = "timezone = \"UTC\"\n\n[idle]\nidle_channel = \"websocket\"\n";
        std::fs::write(dir.path().join("config.toml"), toml).unwrap();
        let cfg = Config::load_at(dir.path()).unwrap();
        assert_eq!(cfg.idle.idle_channel.as_deref(), Some("websocket"));
    }
}
