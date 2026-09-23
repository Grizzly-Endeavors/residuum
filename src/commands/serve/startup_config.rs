//! Decides what startup does with a config that failed to load.

use std::path::Path;

use residuum::config::Config;
use residuum::util::FatalError;

/// Why the config on disk can't start the gateway.
#[derive(Debug)]
pub(super) enum ConfigProblem {
    /// A fresh install that isn't configured yet; the setup wizard handles it.
    NotSetUp,
    /// A config that exists but is malformed or invalid. Carries the error
    /// to show the user.
    Invalid(FatalError),
}

/// Classify a failed [`Config::load_at`] for `config_dir`.
///
/// Only a fresh install whose files parse counts as not set up. A malformed
/// file never does, because the setup wizard would present the user's
/// existing config as if it were gone. The user's files are never modified.
pub(super) fn classify_load_error(config_dir: &Path, err: &FatalError) -> ConfigProblem {
    // The gateway backs up the config each time one loads successfully, so
    // no backup means no config has ever loaded: a fresh install.
    let has_loaded_before = config_dir.join("config.toml.bak").exists();
    if !has_loaded_before && Config::check_files_parse_at(config_dir).is_ok() {
        return ConfigProblem::NotSetUp;
    }
    ConfigProblem::Invalid(invalid_config_error(config_dir, err, has_loaded_before))
}

/// The startup error for a config that exists but can't be loaded: what is
/// wrong, how to fix it, and where the last working config is.
fn invalid_config_error(config_dir: &Path, err: &FatalError, has_backup: bool) -> FatalError {
    let detail = match err {
        FatalError::Config(msg) => msg.clone(),
        other @ (FatalError::Workspace(_) | FatalError::Gateway(_) | FatalError::Other(_)) => {
            other.to_string()
        }
    };
    let backup_hint = if has_backup {
        "\nThe last configuration that loaded successfully is saved next to them as \
         config.toml.bak and providers.toml.bak."
    } else {
        ""
    };
    FatalError::Config(format!(
        "residuum could not start because its configuration is invalid:\n  {detail}\n\n\
         Fix config.toml or providers.toml in {}, then start residuum again.{backup_hint}\n\
         To set it up from scratch instead, run `residuum setup`.",
        config_dir.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROVIDERS: &str = "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n";

    fn config_dir(config_toml: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), config_toml).unwrap();
        std::fs::write(dir.path().join("providers.toml"), PROVIDERS).unwrap();
        dir
    }

    fn classify(dir: &Path) -> ConfigProblem {
        let err = Config::load_at(dir).unwrap_err();
        classify_load_error(dir, &err)
    }

    fn invalid_message(problem: ConfigProblem) -> String {
        match problem {
            ConfigProblem::Invalid(err) => err.to_string(),
            ConfigProblem::NotSetUp => panic!("expected Invalid, got NotSetUp"),
        }
    }

    #[test]
    fn fresh_unconfigured_install_needs_setup() {
        // The bootstrapped config: parses, but has no timezone yet.
        let dir = config_dir("# timezone = \"America/New_York\"\n");
        assert!(matches!(classify(dir.path()), ConfigProblem::NotSetUp));
    }

    #[test]
    fn unknown_key_on_fresh_install_is_invalid_and_names_the_key() {
        let dir = config_dir("timezone = \"UTC\"\ntranscript_retention_days = 30\n");
        let message = invalid_message(classify(dir.path()));
        assert!(
            message.contains("transcript_retention_days"),
            "message should name the bad key: {message}"
        );
        assert!(message.contains("residuum setup"), "{message}");
        assert!(!message.contains(".bak"), "no backup exists yet: {message}");
    }

    #[test]
    fn any_failure_after_a_successful_load_is_invalid() {
        let dir = config_dir("# timezone = \"America/New_York\"\n");
        std::fs::write(dir.path().join("config.toml.bak"), "timezone = \"UTC\"\n").unwrap();
        let message = invalid_message(classify(dir.path()));
        assert!(message.contains("config.toml.bak"), "{message}");
    }

    #[test]
    fn classifying_never_touches_the_users_files() {
        let broken = "timezone = \"UTC\"\nnot_a_real_key = true\n";
        let dir = config_dir(broken);
        std::fs::write(dir.path().join("config.toml.bak"), "timezone = \"UTC\"\n").unwrap();
        let _problem = classify(dir.path());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("config.toml")).unwrap(),
            broken
        );
    }
}
