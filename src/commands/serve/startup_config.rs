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
    // Only reached when there's no last-known-good copy to fall back on
    // either (the caller already checked), so no last-known-good save
    // means no config has ever loaded successfully here: a fresh install.
    let has_loaded_before = residuum::gateway::has_last_known_good(config_dir);
    if !has_loaded_before && Config::check_files_parse_at(config_dir).is_ok() {
        return ConfigProblem::NotSetUp;
    }
    ConfigProblem::Invalid(invalid_config_error(config_dir, err, has_loaded_before))
}

/// The startup error for a config that exists but can't be loaded: what is
/// wrong, how to fix it, and whether a last-known-good copy exists (it
/// still failed to initialize too, or `classify_load_error` wouldn't have
/// been reached — see its caller).
fn invalid_config_error(
    config_dir: &Path,
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
        "\nresiduum also tried the last configuration that worked \
         (config.last-known-good.toml / providers.last-known-good.toml), but it failed to start too."
    } else {
        ""
    };
    FatalError::Config(format!(
        "residuum could not start because its configuration is invalid:\n  {detail}\n\n\
         Fix config.toml or providers.toml in {}, then start residuum again.{lkg_hint}\n\
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
    fn unknown_key_on_fresh_install_no_longer_fails_load() {
        // An unknown key is now skipped with a notice rather than failing
        // the config — see `config::tolerant`. `classify_load_error` is
        // only reached when `Config::load_at` itself failed, so this case
        // doesn't reach it at all any more.
        let dir = config_dir("timezone = \"UTC\"\ntranscript_retention_days = 30\n");
        let cfg = Config::load_at(dir.path()).expect("unknown key should not fail the load");
        assert!(
            cfg.load_notices
                .iter()
                .any(|n| n.contains("transcript_retention_days")),
            "a notice should name the skipped key: {:?}",
            cfg.load_notices
        );
    }

    #[test]
    fn type_error_on_a_known_field_is_invalid_and_names_the_field() {
        let dir = config_dir("timezone = \"UTC\"\nmax_tokens = \"not-a-number\"\n");
        let message = invalid_message(classify(dir.path()));
        assert!(message.contains("residuum setup"), "{message}");
        assert!(!message.contains(".bak"), "no backup exists yet: {message}");
    }

    /// Write a last-known-good pair directly, mirroring what
    /// `gateway::last_known_good::save` would have written after an
    /// earlier successful start.
    fn write_last_known_good(dir: &Path) {
        std::fs::write(
            dir.join("config.last-known-good.toml"),
            "timezone = \"UTC\"\n",
        )
        .unwrap();
        std::fs::write(dir.join("providers.last-known-good.toml"), PROVIDERS).unwrap();
    }

    #[test]
    fn any_failure_after_a_successful_load_is_invalid() {
        let dir = config_dir("# timezone = \"America/New_York\"\n");
        write_last_known_good(dir.path());
        let message = invalid_message(classify(dir.path()));
        assert!(message.contains("last-known-good"), "{message}");
    }

    #[test]
    fn classifying_never_touches_the_users_files() {
        let broken = "timezone = \"UTC\"\nmax_tokens = \"not-a-number\"\n";
        let dir = config_dir(broken);
        write_last_known_good(dir.path());
        let _problem = classify(dir.path());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("config.toml")).unwrap(),
            broken
        );
    }
}
