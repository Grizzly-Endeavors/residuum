//! Hub config loading: the hub counterpart to [`super::load`]'s
//! `Config::load_at`. Reads and tolerantly parses `hub/config.toml`, then
//! resolves it into a [`super::hub_types::HubConfig`].

use std::path::Path;

use crate::util::FatalError;

use super::deserialize;
use super::hub_types::HubConfig;
use super::resolve;
use super::tolerant::parse_tolerating_unknown_keys;

impl HubConfig {
    /// Write default hub config files to `hub_dir` if not already present.
    ///
    /// # Errors
    /// Returns `FatalError::Config` if the directory or files cannot be written.
    pub fn bootstrap_at(hub_dir: &Path) -> Result<(), FatalError> {
        super::bootstrap::bootstrap_hub_at(hub_dir)
    }

    /// Load `hub/config.toml` from `hub_dir`.
    ///
    /// A missing file resolves against defaults (same tolerant-degrade
    /// behavior as the agent config loader): only the timezone is required.
    ///
    /// # Errors
    /// Returns `FatalError::Config` if the file exists but can't be read or
    /// parsed, or if the timezone is missing/invalid.
    pub fn load_at(hub_dir: &Path) -> Result<Self, FatalError> {
        let config_path = hub_dir.join("config.toml");
        let (file, notices) = if config_path.exists() {
            let contents = std::fs::read_to_string(&config_path).map_err(|e| {
                FatalError::Config(format!(
                    "failed to read hub config at {}: {e}",
                    config_path.display()
                ))
            })?;
            let (parsed, notices) = parse_tolerating_unknown_keys::<deserialize::HubConfigFile>(
                &contents,
                "hub/config.toml",
            )
            .map_err(FatalError::Config)?;
            (Some(parsed), notices)
        } else {
            (None, Vec::new())
        };

        let mut cfg = resolve::hub::from_file_and_env(file.as_ref(), hub_dir)?;
        let mut load_notices = notices;
        load_notices.extend(std::mem::take(&mut cfg.load_notices));
        cfg.load_notices = load_notices;

        tracing::info!(
            config = %config_path.display(),
            timezone = %cfg.timezone,
            notices = cfg.load_notices.len(),
            "hub config loaded"
        );
        Ok(cfg)
    }

    /// Load `HubConfig` from an arbitrary hub config directory, bypassing
    /// the default `~/.residuum/hub`. Used by tests and the last-known-good
    /// fallback.
    ///
    /// # Errors
    /// Same as [`Self::load_at`].
    pub fn load_from_path(config_path: &Path, hub_dir: &Path) -> Result<Self, FatalError> {
        if !config_path.exists() {
            return Self::load_at(hub_dir);
        }
        let contents = std::fs::read_to_string(config_path).map_err(|e| {
            FatalError::Config(format!(
                "failed to read hub config at {}: {e}",
                config_path.display()
            ))
        })?;
        let (parsed, notices) = parse_tolerating_unknown_keys::<deserialize::HubConfigFile>(
            &contents,
            "hub/config.toml",
        )
        .map_err(FatalError::Config)?;
        let mut cfg = resolve::hub::from_file_and_env(Some(&parsed), hub_dir)?;
        let mut load_notices = notices;
        load_notices.extend(std::mem::take(&mut cfg.load_notices));
        cfg.load_notices = load_notices;
        Ok(cfg)
    }

    /// Validate a TOML string as `hub/config.toml` without saving it.
    ///
    /// # Errors
    /// Returns a human-readable error string if validation fails.
    pub fn validate_toml(contents: &str, hub_dir: &Path) -> Result<(), String> {
        let (file, _notices) = parse_tolerating_unknown_keys::<deserialize::HubConfigFile>(
            contents,
            "hub/config.toml",
        )?;
        resolve::hub::from_file_and_env(Some(&file), hub_dir).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Diagnostics for `contents` as `hub/config.toml`. Mirrors
    /// `Config::diagnose_agent_toml`'s syntax-then-semantic approach.
    #[must_use]
    pub fn diagnose_toml(contents: &str, hub_dir: &Path) -> Vec<crate::diagnostics::Diagnostic> {
        use crate::diagnostics::{Diagnostic, Location};

        if let Err(e) = toml::from_str::<deserialize::HubConfigFile>(contents) {
            if !e.message().starts_with("unknown field") {
                let location = e
                    .span()
                    .map(|span| Location::from_byte_offset(contents, span.start));
                return vec![match location {
                    Some(loc) => Diagnostic::error_at(e.message().to_string(), loc),
                    None => Diagnostic::error(e.message().to_string()),
                }];
            }
            return match parse_tolerating_unknown_keys::<deserialize::HubConfigFile>(
                contents,
                "hub/config.toml",
            ) {
                Ok((_, notices)) => {
                    let mut diagnostics: Vec<Diagnostic> =
                        notices.into_iter().map(Diagnostic::warning).collect();
                    if let Err(message) = Self::validate_toml(contents, hub_dir) {
                        diagnostics.push(Diagnostic::error(message));
                    }
                    diagnostics
                }
                Err(message) => vec![Diagnostic::error(message)],
            };
        }

        match Self::validate_toml(contents, hub_dir) {
            Ok(()) => Vec::new(),
            Err(message) => vec![Diagnostic::error(message)],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_at_missing_file_errors_on_missing_timezone() {
        let dir = tempfile::tempdir().unwrap();
        let result = HubConfig::load_at(dir.path());
        assert!(result.is_err(), "no config.toml means no timezone set");
    }

    #[test]
    fn load_at_reads_hub_config_toml() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "timezone = \"UTC\"\n").unwrap();
        let cfg = HubConfig::load_at(dir.path()).unwrap();
        assert_eq!(cfg.timezone, chrono_tz::UTC);
        assert_eq!(cfg.config_dir, dir.path());
    }

    #[test]
    fn load_at_unknown_key_produces_notice_not_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "timezone = \"UTC\"\nnot_a_real_key = 1\n",
        )
        .unwrap();
        let cfg = HubConfig::load_at(dir.path()).unwrap();
        assert!(
            cfg.load_notices
                .iter()
                .any(|n| n.contains("not_a_real_key"))
        );
    }
}
