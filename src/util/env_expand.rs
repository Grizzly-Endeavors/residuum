//! `${VAR}` expansion for settings values read from configuration files.

use std::sync::LazyLock;

#[expect(
    clippy::expect_used,
    reason = "hardcoded regex literal is always valid"
)]
static ENV_VAR_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"\$\{([^}:]+?)(?::-(.*?))?\}").expect("hardcoded regex is valid")
});

/// Expand `${VAR}` and `${VAR:-default}` patterns in a string using environment variables.
///
/// Unresolved variables with no default are replaced with an empty string.
#[must_use]
pub(crate) fn expand_env_vars(input: &str) -> String {
    ENV_VAR_RE
        .replace_all(input, |caps: &regex::Captures<'_>| {
            let var_name = caps.get(1).map_or("", |m| m.as_str());
            match std::env::var(var_name) {
                Ok(val) => val,
                Err(_) => caps.get(2).map_or("", |m| m.as_str()).to_string(),
            }
        })
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_env_vars_simple() {
        // SAFETY: test-only, single-threaded test runner for this module
        unsafe { std::env::set_var("TEST_EXPAND_VAR", "hello") };
        assert_eq!(
            expand_env_vars("Bearer ${TEST_EXPAND_VAR}"),
            "Bearer hello",
            "should expand simple env var"
        );
        unsafe { std::env::remove_var("TEST_EXPAND_VAR") };
    }

    #[test]
    fn expand_env_vars_with_default() {
        // SAFETY: test-only, single-threaded test runner for this module
        unsafe { std::env::remove_var("TEST_EXPAND_MISSING") };
        assert_eq!(
            expand_env_vars("${TEST_EXPAND_MISSING:-fallback}"),
            "fallback",
            "should use default when var is missing"
        );
    }

    #[test]
    fn expand_env_vars_no_pattern() {
        assert_eq!(
            expand_env_vars("plain string"),
            "plain string",
            "should pass through strings without patterns"
        );
    }

    #[test]
    fn expand_env_vars_missing_no_default() {
        // SAFETY: test-only, single-threaded test runner for this module
        unsafe { std::env::remove_var("TEST_EXPAND_GONE") };
        assert_eq!(
            expand_env_vars("prefix-${TEST_EXPAND_GONE}-suffix"),
            "prefix--suffix",
            "should replace with empty string when no default"
        );
    }

    #[test]
    fn expand_env_vars_multiple_vars() {
        // SAFETY: test-only, single-threaded test runner for this module
        unsafe { std::env::set_var("TEST_EXPAND_A", "aaa") };
        unsafe { std::env::set_var("TEST_EXPAND_B", "bbb") };
        assert_eq!(
            expand_env_vars("${TEST_EXPAND_A}:${TEST_EXPAND_B}"),
            "aaa:bbb",
            "should expand multiple vars"
        );
        unsafe { std::env::remove_var("TEST_EXPAND_A") };
        unsafe { std::env::remove_var("TEST_EXPAND_B") };
    }

    #[test]
    fn expand_env_vars_empty_var_does_not_use_default() {
        // SAFETY: test-only, single-threaded test runner for this module
        unsafe { std::env::set_var("TEST_EXPAND_EMPTY", "") };
        assert_eq!(
            expand_env_vars("${TEST_EXPAND_EMPTY:-fallback}"),
            "",
            "empty var does not trigger default substitution (diverges from POSIX shell)"
        );
        unsafe { std::env::remove_var("TEST_EXPAND_EMPTY") };
    }
}
