//! Resolution of the hub's `[system_one]` section.

use super::super::constants::TYPESAFE_API_KEY_ENV;
use super::super::deserialize::SystemOneConfigFile;
use super::super::hub_types::{SystemOneConfig, SystemOneProvider};
use super::super::secrets::SecretStore;
use super::resolve_secret_value;

/// Resolve `[system_one]` into an endpoint, or `None` when the section is
/// absent or missing something no default can fill. Every gap is a notice,
/// never a failed load: whatever asks for a decision then reports that no
/// decision model is set up.
pub(super) fn resolve_system_one_config(
    section: Option<&SystemOneConfigFile>,
    secrets: &SecretStore,
    notices: &mut Vec<String>,
) -> Option<SystemOneConfig> {
    let env_key = std::env::var(TYPESAFE_API_KEY_ENV).ok();
    resolve_with_env_key(section, secrets, env_key, notices)
}

/// [`resolve_system_one_config`] with the `TYPESAFE_API_KEY` value passed in.
fn resolve_with_env_key(
    section: Option<&SystemOneConfigFile>,
    secrets: &SecretStore,
    typesafe_env_key: Option<String>,
    notices: &mut Vec<String>,
) -> Option<SystemOneConfig> {
    let section = section?;
    let non_empty = |v: &Option<String>| {
        v.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };

    let Some(provider_raw) = non_empty(&section.provider) else {
        notices.push(
            "[system_one] has no provider — set it to typesafe, ollama, or other to use a decision model."
                .to_string(),
        );
        return None;
    };
    let provider = match provider_raw.parse::<SystemOneProvider>() {
        Ok(p) => p,
        Err(reason) => {
            tracing::warn!(value = %provider_raw, "[system_one] provider is invalid");
            notices.push(format!("[system_one] {reason}."));
            return None;
        }
    };

    let Some(url) = non_empty(&section.url).or_else(|| provider.default_url().map(str::to_string))
    else {
        notices.push(
            "[system_one] uses a custom provider but has no url — add the address of the service."
                .to_string(),
        );
        return None;
    };
    let Some(model) =
        non_empty(&section.model).or_else(|| provider.default_model().map(str::to_string))
    else {
        notices.push(format!(
            "[system_one] has no model — choose which {} model to use.",
            provider.display_name()
        ));
        return None;
    };

    let api_key = match non_empty(&section.api_key) {
        Some(raw) => {
            let resolved = resolve_secret_value(&raw, secrets);
            if resolved.is_none() {
                tracing::warn!(reference = %raw, "[system_one] api_key reference did not resolve");
                notices.push(format!(
                    "[system_one] api_key {raw} couldn't be found — save the key under Saved keys."
                ));
            }
            resolved
        }
        None if provider == SystemOneProvider::TypeSafe => {
            let from_env = typesafe_env_key.filter(|v| !v.is_empty());
            if from_env.is_none() {
                notices.push(
                    "[system_one] TypeSafe needs an API key — add one under Settings → All agents → Decision model."
                        .to_string(),
                );
            }
            from_env
        }
        None => None,
    };

    Some(SystemOneConfig {
        provider,
        url,
        model,
        api_key,
        keep_alive: non_empty(&section.keep_alive),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Resolve `toml_text` as a `[system_one]` section against a secret store
    /// holding `typesafe = "sk-1"`.
    fn resolve(toml_text: &str, env_key: Option<&str>) -> (Option<SystemOneConfig>, Vec<String>) {
        let dir = tempfile::tempdir().unwrap();
        let mut secrets = SecretStore::load(dir.path()).unwrap();
        secrets.set("typesafe", "sk-1", dir.path()).unwrap();
        let section: SystemOneConfigFile = toml::from_str(toml_text).unwrap();
        let mut notices = Vec::new();
        let cfg = resolve_with_env_key(
            Some(&section),
            &secrets,
            env_key.map(str::to_string),
            &mut notices,
        );
        (cfg, notices)
    }

    #[test]
    fn absent_section_is_none_without_notices() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = SecretStore::load(dir.path()).unwrap();
        let mut notices = Vec::new();
        assert!(resolve_system_one_config(None, &secrets, &mut notices).is_none());
        assert!(notices.is_empty(), "no section is not a problem");
    }

    #[test]
    fn typesafe_fills_url_and_model_defaults() {
        let (cfg, notices) = resolve(
            "provider = \"typesafe\"\napi_key = \"secret:typesafe\"\n",
            None,
        );
        let cfg = cfg.unwrap();
        assert_eq!(cfg.provider, SystemOneProvider::TypeSafe);
        assert_eq!(cfg.url, "https://api.typesafe.ai");
        assert_eq!(cfg.model, "jev-latest");
        assert_eq!(cfg.api_key.as_deref(), Some("sk-1"));
        assert!(notices.is_empty(), "{notices:?}");
    }

    #[test]
    fn typesafe_key_falls_back_to_env() {
        let (cfg, notices) = resolve("provider = \"typesafe\"\n", Some("env-key"));
        assert_eq!(cfg.unwrap().api_key.as_deref(), Some("env-key"));
        assert!(notices.is_empty(), "{notices:?}");
    }

    #[test]
    fn typesafe_without_any_key_still_builds_with_a_notice() {
        let (cfg, notices) = resolve("provider = \"typesafe\"\n", None);
        assert!(cfg.is_some(), "a missing key still builds the endpoint");
        assert!(notices.iter().any(|n| n.contains("API key")), "{notices:?}");
    }

    #[test]
    fn unresolved_secret_reference_is_a_notice() {
        let (cfg, notices) = resolve(
            "provider = \"typesafe\"\napi_key = \"secret:missing\"\n",
            None,
        );
        assert!(cfg.unwrap().api_key.is_none());
        assert!(
            notices.iter().any(|n| n.contains("secret:missing")),
            "{notices:?}"
        );
    }

    #[test]
    fn ollama_without_model_is_none_with_a_notice() {
        let (cfg, notices) = resolve("provider = \"ollama\"\n", None);
        assert!(cfg.is_none());
        assert!(
            notices.iter().any(|n| n.contains("no model")),
            "{notices:?}"
        );
    }

    #[test]
    fn ollama_defaults_to_localhost() {
        let (cfg, _) = resolve(
            "provider = \"ollama\"\nmodel = \"nimble\"\nkeep_alive = \"5m\"\n",
            None,
        );
        let cfg = cfg.unwrap();
        assert_eq!(cfg.url, "http://localhost:11434");
        assert_eq!(cfg.keep_alive.as_deref(), Some("5m"));
        assert!(cfg.api_key.is_none());
    }

    #[test]
    fn other_without_url_is_none_with_a_notice() {
        let (cfg, notices) = resolve("provider = \"other\"\nmodel = \"m\"\n", None);
        assert!(cfg.is_none());
        assert!(notices.iter().any(|n| n.contains("no url")), "{notices:?}");
    }

    #[test]
    fn other_with_url_and_model_uses_the_host_as_its_name() {
        let (cfg, notices) = resolve(
            "provider = \"other\"\nurl = \"https://decide.example.com\"\nmodel = \"clef\"\n",
            None,
        );
        assert_eq!(cfg.unwrap().display_name(), "decide.example.com");
        assert!(notices.is_empty(), "{notices:?}");
    }

    #[test]
    fn unknown_provider_is_a_notice() {
        let (cfg, notices) = resolve("provider = \"jevvy\"\n", None);
        assert!(cfg.is_none());
        assert!(notices.iter().any(|n| n.contains("jevvy")), "{notices:?}");
    }
}
