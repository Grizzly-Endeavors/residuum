//! Agent config loading and validation methods: the agent counterpart to
//! [`super::hub_load`]'s `HubConfig::load_at`.

use std::path::{Path, PathBuf};

use crate::util::FatalError;

use super::tolerant::parse_tolerating_unknown_keys;
use super::{Config, HubConfig};
use super::{bootstrap, deserialize, resolve};

impl Config {
    /// Create an agent's `config/` directory and regenerate its reference
    /// templates.
    ///
    /// - `config.example.toml`/`providers.example.toml` are always regenerated (kept in
    ///   sync with the current schema).
    /// - The live `config.toml`/`providers.toml` are never written here: an
    ///   agent is discovered by its `config/config.toml`, so the caller writes it
    ///   last, once the real configuration is complete.
    ///
    /// # Errors
    /// Returns `FatalError::Config` if the directory or files cannot be written.
    pub fn bootstrap_agent_config_dir(agent_config_dir: &Path) -> Result<(), FatalError> {
        bootstrap::bootstrap_agent_at(agent_config_dir)
    }

    /// Load an agent's configuration from `agent_dir` (its workspace root)
    /// and the hub config it belongs to.
    ///
    /// The agent's own `config.toml`/`providers.toml` live in
    /// `agent_dir/config/`. The agent's name is `agent_dir`'s directory name.
    ///
    /// # Errors
    /// Returns `FatalError::Config` if `providers.toml` is missing, either
    /// file exists but cannot be read or parsed, or required values are
    /// missing/invalid.
    #[tracing::instrument(skip_all, fields(agent_dir = %agent_dir.display()))]
    pub fn load_agent_at(agent_dir: &Path, hub: &HubConfig) -> Result<Self, FatalError> {
        let agent_config_dir = agent_dir.join("config");
        let config_path = agent_config_dir.join("config.toml");
        let providers_path = find_providers_path(&agent_config_dir)?;
        let agent_name = agent_name_from_dir(agent_dir)?;
        Self::load_agent_from_paths(agent_dir, &config_path, &providers_path, &agent_name, hub)
    }

    /// Load an agent's configuration from explicit `config.toml`/`providers.toml`
    /// paths.
    ///
    /// Used by [`load_agent_at`](Self::load_agent_at) for the normal path, and by
    /// the last-known-good fallback (`gateway::last_known_good`) to load a
    /// saved copy under different filenames without touching the user's
    /// live `config.toml`/`providers.toml`.
    ///
    /// # Errors
    /// Returns `FatalError::Config` if either file exists but cannot be
    /// read or parsed, or if required values are missing.
    pub(crate) fn load_agent_from_paths(
        agent_dir: &Path,
        config_path: &Path,
        providers_path: &Path,
        agent_name: &str,
        hub: &HubConfig,
    ) -> Result<Self, FatalError> {
        let (file_config, config_notices) = if config_path.exists() {
            let contents = std::fs::read_to_string(config_path).map_err(|e| {
                FatalError::Config(format!(
                    "failed to read config at {}: {e}",
                    config_path.display()
                ))
            })?;
            let (parsed, notices) = parse_tolerating_unknown_keys::<deserialize::AgentConfigFile>(
                &contents,
                "config.toml",
            )
            .map_err(FatalError::Config)?;
            (Some(parsed), notices)
        } else {
            (None, Vec::new())
        };

        let (providers, providers_notices) = load_providers(providers_path)?;

        let mut cfg = resolve::from_file_and_env(
            file_config.as_ref(),
            Some(&providers),
            agent_dir,
            agent_name,
            hub,
        )?;
        let mut load_notices = config_notices;
        load_notices.extend(providers_notices);
        load_notices.extend(std::mem::take(&mut cfg.load_notices));
        cfg.load_notices = load_notices;

        tracing::info!(
            agent = %agent_name,
            config = %config_path.display(),
            providers = %providers_path.display(),
            main_model = %cfg.main.first().map(|p| p.model.to_string()).unwrap_or_default(),
            notices = cfg.load_notices.len(),
            "agent config loaded"
        );
        Ok(cfg)
    }

    /// Check that an agent's `config.toml` and `providers.toml` are
    /// well-formed: valid TOML containing only known keys.
    ///
    /// Missing files pass. Semantic checks (required values, model specs)
    /// are left to [`load_agent_at`](Self::load_agent_at), so this separates a
    /// malformed file from one that is merely not configured yet.
    ///
    /// # Errors
    /// Returns `FatalError::Config` naming the file and the parse error.
    pub fn check_files_parse_at(agent_config_dir: &Path) -> Result<(), FatalError> {
        read_optional_toml::<deserialize::AgentConfigFile>(
            &agent_config_dir.join("config.toml"),
            "config.toml",
        )
        .map_err(FatalError::Config)?;
        read_optional_toml::<deserialize::ProvidersFile>(
            &agent_config_dir.join("providers.toml"),
            "providers.toml",
        )
        .map_err(FatalError::Config)?;
        Ok(())
    }

    /// Validate a TOML string as an agent's `config.toml` without saving it.
    ///
    /// Parses the TOML into the raw config structure, then runs full resolution
    /// to catch semantic errors. Reads the existing `providers.toml` from the
    /// agent's config directory for model resolution.
    ///
    /// # Errors
    /// Returns a human-readable error string if validation fails.
    pub fn validate_agent_toml(
        contents: &str,
        agent_dir: &Path,
        agent_name: &str,
        hub: &HubConfig,
    ) -> Result<(), String> {
        let (file, _notices) =
            parse_tolerating_unknown_keys::<deserialize::AgentConfigFile>(contents, "config.toml")?;

        let providers_file = read_optional_toml::<deserialize::ProvidersFile>(
            &agent_dir.join("config").join("providers.toml"),
            "providers.toml",
        )?;

        resolve::from_file_and_env(
            Some(&file),
            providers_file.as_ref(),
            agent_dir,
            agent_name,
            hub,
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Validate a TOML string as an agent's `providers.toml` without saving it.
    ///
    /// Parses the TOML and runs model resolution against the existing
    /// `config.toml` on disk to catch semantic errors.
    ///
    /// # Errors
    /// Returns a human-readable error string if validation fails.
    pub fn validate_agent_providers_toml(
        contents: &str,
        agent_dir: &Path,
        agent_name: &str,
        hub: &HubConfig,
    ) -> Result<(), String> {
        let (providers_file, _notices) = parse_tolerating_unknown_keys::<deserialize::ProvidersFile>(
            contents,
            "providers.toml",
        )?;

        let config_file = read_optional_toml::<deserialize::AgentConfigFile>(
            &agent_dir.join("config").join("config.toml"),
            "config.toml",
        )?;

        resolve::from_file_and_env(
            config_file.as_ref(),
            Some(&providers_file),
            agent_dir,
            agent_name,
            hub,
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Diagnostics for `contents` as an agent's `config.toml`.
    ///
    /// Reuses [`Self::validate_agent_toml`] for the semantic check, so a
    /// diagnostic can never disagree with what loading rejects. Parses the
    /// TOML a second time first so a syntax error can carry the parser's own
    /// line/column via its byte span — `validate_agent_toml` only returns
    /// the crate's pre-formatted `Display` text, which has no structured
    /// position. Semantic errors have no byte position to report, since
    /// they're found only after deserialization succeeds.
    #[must_use]
    pub fn diagnose_agent_toml(
        contents: &str,
        agent_dir: &Path,
        agent_name: &str,
        hub: &HubConfig,
    ) -> Vec<crate::diagnostics::Diagnostic> {
        diagnose_toml_file::<deserialize::AgentConfigFile>(contents, "config.toml", || {
            Self::validate_agent_toml(contents, agent_dir, agent_name, hub)
        })
    }

    /// Diagnostics for `contents` as an agent's `providers.toml`. See
    /// [`Self::diagnose_agent_toml`] for the approach.
    #[must_use]
    pub fn diagnose_agent_providers_toml(
        contents: &str,
        agent_dir: &Path,
        agent_name: &str,
        hub: &HubConfig,
    ) -> Vec<crate::diagnostics::Diagnostic> {
        diagnose_toml_file::<deserialize::ProvidersFile>(contents, "providers.toml", || {
            Self::validate_agent_providers_toml(contents, agent_dir, agent_name, hub)
        })
    }

    /// Build `CompletionOptions` for a named role, applying per-role overrides
    /// over the global defaults.
    #[must_use]
    pub fn completion_options_for_role(&self, role: &str) -> crate::inference::CompletionOptions {
        let ov = self.role_overrides.get(role);
        crate::inference::CompletionOptions {
            max_tokens: Some(self.max_tokens),
            temperature: ov.and_then(|o| o.temperature).or(self.temperature),
            thinking: ov
                .and_then(|o| o.thinking.clone())
                .or(self.thinking.clone()),
            ..crate::inference::CompletionOptions::default()
        }
    }
}

/// The agent's name from its workspace directory's file name.
///
/// # Errors
/// Returns `FatalError::Config` if `agent_dir` has no valid UTF-8 file name
/// component (shouldn't happen for a directory reached through agent
/// discovery, which already filters on this).
fn agent_name_from_dir(agent_dir: &Path) -> Result<String, FatalError> {
    agent_dir
        .file_name()
        .and_then(|n| n.to_str())
        .map(str::to_string)
        .ok_or_else(|| {
            FatalError::Config(format!(
                "couldn't determine an agent name from directory {}",
                agent_dir.display()
            ))
        })
}

/// Locate the `providers.toml` to load for the given agent config directory.
///
/// # Errors
/// Returns `FatalError::Config` if no providers file can be found.
fn find_providers_path(agent_config_dir: &Path) -> Result<PathBuf, FatalError> {
    let providers_path = agent_config_dir.join("providers.toml");
    if providers_path.exists() {
        return Ok(providers_path);
    }

    Err(FatalError::Config(format!(
        "providers.toml not found at {}; run 'residuum setup' to create it",
        providers_path.display()
    )))
}

/// Read and parse a TOML file if it exists, returning `None` if absent.
///
/// # Errors
/// Returns a human-readable error string if the file cannot be read or parsed.
fn read_optional_toml<T: serde::de::DeserializeOwned>(
    path: &Path,
    file_name: &str,
) -> Result<Option<T>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let contents =
        std::fs::read_to_string(path).map_err(|e| format!("failed to read {file_name}: {e}"))?;
    let parsed =
        toml::from_str::<T>(&contents).map_err(|e| format!("{file_name} parse error: {e}"))?;
    Ok(Some(parsed))
}

/// Shared syntax-then-semantic diagnostic path for a TOML config file: try
/// to parse `contents` as `T` first so a syntax error carries the parser's
/// own line/column (from its byte span). An unknown top-level key is not a
/// syntax error here — [`load_agent_at`](Config::load_agent_at) tolerates it
/// (see [`parse_tolerating_unknown_keys`]), skipping the key with a notice
/// instead of failing the file — so it's reported as a warning, one per
/// dropped key, not an error. Once the file parses (with or without keys
/// dropped), `validate` (the real semantic check loading also runs) finds
/// the one error loading would actually raise.
fn diagnose_toml_file<T: serde::de::DeserializeOwned>(
    contents: &str,
    file_label: &str,
    validate: impl FnOnce() -> Result<(), String>,
) -> Vec<crate::diagnostics::Diagnostic> {
    use crate::diagnostics::{Diagnostic, Location};

    if let Err(e) = toml::from_str::<T>(contents) {
        if !e.message().starts_with("unknown field") {
            let location = e
                .span()
                .map(|span| Location::from_byte_offset(contents, span.start));
            return vec![match location {
                Some(loc) => Diagnostic::error_at(e.message().to_string(), loc),
                None => Diagnostic::error(e.message().to_string()),
            }];
        }

        // At least one unknown key is present; re-parse tolerating it (and
        // any others) to collect one warning per dropped key, matching what
        // loading actually does with this content.
        return match parse_tolerating_unknown_keys::<T>(contents, file_label) {
            Ok((_, notices)) => {
                let mut diagnostics: Vec<Diagnostic> =
                    notices.into_iter().map(Diagnostic::warning).collect();
                if let Err(message) = validate() {
                    diagnostics.push(Diagnostic::error(message));
                }
                diagnostics
            }
            Err(message) => vec![Diagnostic::error(message)],
        };
    }

    match validate() {
        Ok(()) => Vec::new(),
        Err(message) => vec![Diagnostic::error(message)],
    }
}

/// Load and parse a `providers.toml` file from the given path.
fn load_providers(path: &Path) -> Result<(deserialize::ProvidersFile, Vec<String>), FatalError> {
    let contents = std::fs::read_to_string(path).map_err(|e| {
        FatalError::Config(format!(
            "failed to read providers config at {}: {e}",
            path.display()
        ))
    })?;
    parse_tolerating_unknown_keys::<deserialize::ProvidersFile>(&contents, "providers.toml")
        .map_err(FatalError::Config)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Valid minimal agent config TOML (no timezone — that's hub-level now).
    const VALID_CONFIG: &str = "";

    /// Valid minimal providers TOML.
    const VALID_PROVIDERS: &str = "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n";

    fn test_hub() -> HubConfig {
        HubConfig {
            timezone: chrono_tz::UTC,
            gateway: super::super::GatewayConfig::default(),
            cloud: None,
            a2a: super::super::HubA2aConfig::default(),
            tracing: super::super::TracingConfig::default(),
            background: super::super::HubBackgroundConfig::default(),
            config_dir: std::env::temp_dir().join("residuum-test-load-hub"),
            load_notices: Vec::new(),
        }
    }

    /// Write a bootstrapped agent directory (`config/config.toml`,
    /// `config/providers.toml`) under `dir/<name>`, returning the agent dir.
    fn write_agent(dir: &Path, name: &str, config: &str, providers: &str) -> PathBuf {
        let agent_dir = dir.join(name);
        let config_dir = agent_dir.join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(config_dir.join("config.toml"), config).unwrap();
        std::fs::write(config_dir.join("providers.toml"), providers).unwrap();
        agent_dir
    }

    #[test]
    fn validate_agent_toml_accepts_valid_config() {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = write_agent(dir.path(), "sam", VALID_CONFIG, VALID_PROVIDERS);
        let result = Config::validate_agent_toml(VALID_CONFIG, &agent_dir, "sam", &test_hub());
        assert!(result.is_ok(), "valid config should pass: {result:?}");
    }

    #[test]
    fn validate_agent_toml_resolves_secrets_from_hub_store() {
        let dir = tempfile::tempdir().unwrap();
        let hub = test_hub();
        std::fs::create_dir_all(&hub.config_dir).unwrap();
        let mut store = super::super::secrets::SecretStore::load(&hub.config_dir).unwrap();
        store
            .set("test_api_key", "sk-test-123", &hub.config_dir)
            .unwrap();

        let providers_with_secret = r#"
[providers.my-provider]
type = "anthropic"
api_key = "secret:test_api_key"

[models]
main = "my-provider/claude-sonnet-4-6"
"#;
        let agent_dir = write_agent(dir.path(), "sam", VALID_CONFIG, providers_with_secret);

        let result = Config::validate_agent_toml(VALID_CONFIG, &agent_dir, "sam", &hub);
        assert!(
            result.is_ok(),
            "secret reference should resolve with real store: {result:?}"
        );
    }

    #[test]
    fn load_agent_at_returns_error_on_invalid_toml() {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = write_agent(
            dir.path(),
            "sam",
            "invalid toml syntax = [",
            VALID_PROVIDERS,
        );

        let result = Config::load_agent_at(&agent_dir, &test_hub());
        assert!(
            result.is_err(),
            "load_agent_at should fail on invalid TOML syntax"
        );
        let err = result.unwrap_err();
        assert!(
            matches!(err, FatalError::Config(_)),
            "error should be of type FatalError::Config, got: {err:?}"
        );
    }

    #[test]
    fn load_agent_at_returns_error_when_providers_missing() {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = dir.path().join("sam");
        std::fs::create_dir_all(agent_dir.join("config")).unwrap();
        std::fs::write(agent_dir.join("config/config.toml"), VALID_CONFIG).unwrap();

        let result = Config::load_agent_at(&agent_dir, &test_hub());
        assert!(
            result.is_err(),
            "load_agent_at should fail when providers.toml is missing"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("providers.toml"),
            "error should mention providers.toml: {err}"
        );
    }

    #[test]
    fn load_agent_at_uses_the_directory_name_as_the_agent_name() {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = write_agent(dir.path(), "sam", VALID_CONFIG, VALID_PROVIDERS);
        let cfg = Config::load_agent_at(&agent_dir, &test_hub()).unwrap();
        assert_eq!(cfg.agent_name, "sam");
        assert_eq!(cfg.workspace_dir, agent_dir);
        assert_eq!(cfg.config_dir, agent_dir.join("config"));
    }

    #[test]
    fn validate_agent_toml_rejects_invalid_toml_syntax() {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = write_agent(dir.path(), "sam", VALID_CONFIG, VALID_PROVIDERS);
        let bad_toml = "this is not valid toml";
        let result = Config::validate_agent_toml(bad_toml, &agent_dir, "sam", &test_hub());
        assert!(result.is_err(), "invalid TOML syntax should fail parse");
        let err = result.unwrap_err();
        assert!(
            err.contains("TOML parse error"),
            "error should mention TOML parse error: {err}"
        );
    }

    #[test]
    fn validate_agent_providers_toml_rejects_invalid_model_format() {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = write_agent(dir.path(), "sam", VALID_CONFIG, VALID_PROVIDERS);

        let bad_providers = "[models]\nmain = \"invalid-format\"\n";
        let result =
            Config::validate_agent_providers_toml(bad_providers, &agent_dir, "sam", &test_hub());
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
    fn load_providers_helper_parses_valid_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("providers.toml");
        std::fs::write(&path, VALID_PROVIDERS).unwrap();
        let result = super::load_providers(&path);
        assert!(result.is_ok(), "valid providers should parse: {result:?}");
        let (providers, notices) = result.unwrap();
        assert!(
            providers.models.is_some(),
            "parsed providers should have models section"
        );
        assert!(notices.is_empty(), "valid file should have no notices");
    }

    #[test]
    fn load_providers_helper_fails_on_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.toml");
        let result = super::load_providers(&path);
        assert!(result.is_err(), "missing file should fail");
    }

    #[test]
    fn diagnose_agent_toml_reports_syntax_error_with_line_column() {
        use crate::diagnostics::Location;

        let dir = tempfile::tempdir().unwrap();
        let agent_dir = write_agent(dir.path(), "sam", VALID_CONFIG, VALID_PROVIDERS);
        let diagnostics =
            Config::diagnose_agent_toml("this is not valid toml", &agent_dir, "sam", &test_hub());
        assert_eq!(diagnostics.len(), 1, "should report exactly one diagnostic");
        assert!(
            matches!(
                diagnostics.first().unwrap().location,
                Some(Location::LineColumn { .. })
            ),
            "TOML syntax error should carry a line/column: {diagnostics:?}"
        );
    }

    #[test]
    fn diagnose_agent_toml_reports_unknown_key_as_warning() {
        use crate::diagnostics::Severity;

        let dir = tempfile::tempdir().unwrap();
        let agent_dir = write_agent(dir.path(), "sam", VALID_CONFIG, VALID_PROVIDERS);
        let contents = "not_a_real_key = 1\n";
        let diagnostics = Config::diagnose_agent_toml(contents, &agent_dir, "sam", &test_hub());
        assert_eq!(diagnostics.len(), 1, "should report exactly one diagnostic");
        let diagnostic = diagnostics.first().unwrap();
        assert_eq!(diagnostic.severity, Severity::Warning);
        assert!(
            diagnostic.message.contains("not_a_real_key"),
            "{}",
            diagnostic.message
        );
    }

    #[test]
    fn diagnose_agent_toml_clean_config_has_no_diagnostics() {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = write_agent(dir.path(), "sam", VALID_CONFIG, VALID_PROVIDERS);
        assert!(
            Config::diagnose_agent_toml(VALID_CONFIG, &agent_dir, "sam", &test_hub()).is_empty()
        );
    }

    #[test]
    fn diagnose_agent_providers_toml_reports_semantic_error() {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = write_agent(dir.path(), "sam", VALID_CONFIG, VALID_PROVIDERS);
        let bad_providers = "[models]\nmain = \"invalid-format\"\n";
        let diagnostics =
            Config::diagnose_agent_providers_toml(bad_providers, &agent_dir, "sam", &test_hub());
        assert_eq!(diagnostics.len(), 1);
        assert!(
            diagnostics
                .first()
                .unwrap()
                .message
                .contains("expected 'provider/model' format")
        );
    }

    #[test]
    fn idle_config_defaults_when_section_missing() {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = write_agent(dir.path(), "sam", VALID_CONFIG, VALID_PROVIDERS);
        let cfg = Config::load_agent_at(&agent_dir, &test_hub()).unwrap();
        assert_eq!(cfg.idle.timeout, std::time::Duration::from_mins(30));
        assert!(cfg.idle.idle_channel.is_none());
    }

    #[test]
    fn idle_config_timeout_zero_disables() {
        let dir = tempfile::tempdir().unwrap();
        let agent_dir = write_agent(
            dir.path(),
            "sam",
            "[idle]\ntimeout_minutes = 0\n",
            VALID_PROVIDERS,
        );
        let cfg = Config::load_agent_at(&agent_dir, &test_hub()).unwrap();
        assert_eq!(cfg.idle.timeout, std::time::Duration::ZERO);
    }

    #[test]
    fn idle_config_explicit_values() {
        let dir = tempfile::tempdir().unwrap();
        let toml = "[telegram]\ntoken = \"test-token\"\n\n[idle]\ntimeout_minutes = 15\nidle_channel = \"telegram\"\n";
        let agent_dir = write_agent(dir.path(), "sam", toml, VALID_PROVIDERS);
        let cfg = Config::load_agent_at(&agent_dir, &test_hub()).unwrap();
        assert_eq!(cfg.idle.timeout, std::time::Duration::from_mins(15));
        assert_eq!(cfg.idle.idle_channel.as_deref(), Some("telegram"));
    }
}
