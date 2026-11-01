//! Hub-level config resolution: timezone, gateway, cloud, the A2A listener,
//! tracing, Web Push, the shared background limits and the model price
//! overrides — everything that lives in `hub/config.toml` rather than an
//! agent's own config.

use std::collections::BTreeMap;
use std::path::Path;
use std::str::FromStr;

use residuum_core::util::FatalError;

use super::super::deserialize::{HubBackgroundConfigFile, HubConfigFile, HubPushConfigFile};
use super::super::hub_types::{
    HubA2aConfig, HubBackgroundConfig, HubConfig, HubPushConfig, ModelPrice,
};
use super::super::provider::ModelSpec;
use super::gateway;
use super::load_secrets_degraded;
use super::tracing_config;

/// Build a `HubConfig` from an optional hub config file and environment
/// variables.
///
/// # Errors
/// Returns `FatalError::Config` if the timezone is missing/invalid or the
/// tracing log level string is invalid.
pub fn from_file_and_env(
    file: Option<&HubConfigFile>,
    hub_dir: &Path,
) -> Result<HubConfig, FatalError> {
    from_file_and_env_inner(file, hub_dir, false)
}

/// Like [`from_file_and_env`], but a missing timezone does not fail the load.
///
/// The first-run `hub/config.toml` leaves `timezone` commented out, and the
/// setup wizard is what writes it. Requiring one before the hub is listening
/// means the wizard never starts. An invalid name is still an error: that
/// file was edited, and guessing would hide the typo. The stand-in is the
/// machine's timezone and lives only in this loaded config.
///
/// # Errors
/// Returns `FatalError::Config` if the timezone name is invalid or the
/// tracing log level string is invalid.
pub(crate) fn from_file_and_env_for_onboarding(
    file: Option<&HubConfigFile>,
    hub_dir: &Path,
) -> Result<HubConfig, FatalError> {
    from_file_and_env_inner(file, hub_dir, true)
}

fn from_file_and_env_inner(
    file: Option<&HubConfigFile>,
    hub_dir: &Path,
    allow_unset_timezone: bool,
) -> Result<HubConfig, FatalError> {
    let mut notices: Vec<String> = Vec::new();
    let secrets = load_secrets_degraded(hub_dir, &mut notices);

    let timezone = match gateway::resolve_timezone(file) {
        Ok(tz) => tz,
        Err(err) if allow_unset_timezone && gateway::is_missing_timezone(&err) => {
            let stand_in = gateway::machine_timezone();
            tracing::info!(
                timezone = %stand_in,
                "no timezone configured; the setup wizard will ask for one. times use the machine timezone until then"
            );
            stand_in
        }
        Err(err) => return Err(err),
    };
    let gateway = gateway::resolve_gateway_config(file.and_then(|f| f.gateway.as_ref()));
    let cloud = gateway::resolve_cloud_config(file.and_then(|f| f.cloud.as_ref()), &secrets);
    let tracing = tracing_config::resolve_tracing_config(file.and_then(|f| f.tracing.as_ref()))?;
    let (a2a_enabled, a2a_port, a2a_public_url) =
        super::a2a::resolve_hub_a2a_config(file.and_then(|f| f.a2a.as_ref()));
    let a2a = HubA2aConfig {
        enabled: a2a_enabled,
        port: a2a_port,
        public_url: a2a_public_url,
    };
    let background = resolve_hub_background_config(file.and_then(|f| f.background.as_ref()));
    let push = resolve_hub_push_config(file.and_then(|f| f.push.as_ref()), &mut notices);
    let system_one = super::system_one::resolve_system_one_config(
        file.and_then(|f| f.system_one.as_ref()),
        &secrets,
        &mut notices,
    );
    let pricing = resolve_hub_pricing(file.and_then(|f| f.pricing.as_ref()), &mut notices);

    Ok(HubConfig {
        timezone,
        gateway,
        cloud,
        a2a,
        tracing,
        background,
        push,
        system_one,
        pricing,
        config_dir: hub_dir.to_path_buf(),
        load_notices: notices,
    })
}

/// Resolve the hub's `[push]` section.
///
/// A `contact` that isn't a `mailto:` address or an `https:` URL is dropped
/// with a notice, so push keeps working under the default contact until the
/// user fixes the value (the same degradation an invalid A2A visibility gets).
fn resolve_hub_push_config(
    section: Option<&HubPushConfigFile>,
    notices: &mut Vec<String>,
) -> HubPushConfig {
    let contact = section
        .and_then(|s| s.contact.as_deref())
        .map(str::trim)
        .filter(|v| !v.is_empty());
    let Some(contact) = contact else {
        return HubPushConfig::default();
    };
    match validate_push_contact(contact) {
        Ok(()) => HubPushConfig {
            contact: Some(contact.to_string()),
        },
        Err(reason) => {
            tracing::warn!(value = contact, %reason, "[push] contact is invalid, using the default");
            notices.push(format!(
                "[push] contact {reason} — push services will see the default contact until you fix it."
            ));
            HubPushConfig::default()
        }
    }
}

/// Check that `contact` is what a push service accepts in the VAPID `sub`
/// claim: a `mailto:` address or an `https:` URL. The error completes the
/// sentence "\[push\] contact ...".
fn validate_push_contact(contact: &str) -> Result<(), String> {
    if let Some(address) = contact.strip_prefix("mailto:") {
        let well_formed = address
            .split_once('@')
            .is_some_and(|(user, host)| !user.is_empty() && host.contains('.'))
            && !address.contains(char::is_whitespace);
        return if well_formed {
            Ok(())
        } else {
            Err(format!(
                "must be a mailto: link with an email address, such as mailto:you@example.com, got \"{contact}\""
            ))
        };
    }
    match url::Url::parse(contact) {
        Ok(url) if url.scheme() == "https" && url.host_str().is_some() => Ok(()),
        _ => Err(format!(
            "must be a mailto: link or an https:// address, got \"{contact}\""
        )),
    }
}

/// Resolve the hub's shared background limits (session budget, hop limits).
fn resolve_hub_background_config(section: Option<&HubBackgroundConfigFile>) -> HubBackgroundConfig {
    let mut cfg = HubBackgroundConfig::default();
    let Some(section) = section else {
        return cfg;
    };
    if let Some(v) = section.max_concurrent {
        cfg.max_concurrent = v;
    }
    if let Some(v) = section.hop_soft_limit {
        cfg.hop_soft_limit = v;
    }
    if let Some(v) = section.hop_hard_limit {
        cfg.hop_hard_limit = v;
    }
    cfg
}

/// Resolve the hub's `[pricing]` overrides. A key must name a provider kind
/// and a model, and each rate must be a finite, non-negative number of USD per
/// million tokens. An entry that breaks either rule is dropped with a notice,
/// so the model keeps its built-in price (or none) and the other entries load.
fn resolve_hub_pricing(
    section: Option<&BTreeMap<String, ModelPrice>>,
    notices: &mut Vec<String>,
) -> BTreeMap<String, ModelPrice> {
    let Some(section) = section else {
        return BTreeMap::new();
    };
    section
        .iter()
        .filter_map(|(key, price)| match validate_price(key, price) {
            Ok(()) => Some((key.clone(), price.clone())),
            Err(reason) => {
                tracing::warn!(key = %key, %reason, "[pricing] entry is invalid, ignoring it");
                notices.push(format!(
                    "[pricing] \"{key}\" {reason}; this price is ignored."
                ));
                None
            }
        })
        .collect()
}

/// Check one `[pricing]` entry. The error completes the sentence
/// `[pricing] "key" ...`.
fn validate_price(key: &str, price: &ModelPrice) -> Result<(), String> {
    ModelSpec::from_str(key)?;
    let rates = [
        ("input", price.input),
        ("output", price.output),
        ("cache_read", price.cache_read),
        ("cache_write", price.cache_write),
        ("cache_write_long", price.cache_write_long),
    ];
    for (rate, value) in rates {
        if !value.is_finite() || value < 0.0 {
            return Err(format!(
                "{rate} must be a finite number of zero or more USD per million tokens, got {value}"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
#[expect(
    unsafe_code,
    reason = "std::env::set_var/remove_var require unsafe in edition 2024"
)]
mod tests {
    use super::super::test_helpers::ENV_MUTEX;
    use super::*;

    fn parse(toml: &str) -> HubConfigFile {
        toml::from_str(toml).unwrap()
    }

    const FULL_PRICE: &str =
        "input = 1.0\noutput = 5.0\ncache_read = 0.1\ncache_write = 1.25\ncache_write_long = 2.0\n";

    fn test_dir() -> std::path::PathBuf {
        std::env::temp_dir().join("residuum-test-hub-config")
    }

    #[test]
    fn timezone_required() {
        let result = from_file_and_env(None, &test_dir());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("timezone"));
    }

    #[test]
    fn defaults_resolve() {
        let file = parse("timezone = \"UTC\"\n");
        let cfg = from_file_and_env(Some(&file), &test_dir()).unwrap();
        assert_eq!(cfg.timezone, chrono_tz::UTC);
        assert!(cfg.a2a.enabled);
        assert_eq!(cfg.a2a.port, crate::DEFAULT_A2A_PORT);
        assert_eq!(cfg.background.max_concurrent, 3);
        assert_eq!(cfg.background.hop_soft_limit, 8);
        assert_eq!(cfg.background.hop_hard_limit, 32);
        assert!(cfg.cloud.is_none());
    }

    #[test]
    fn gateway_and_a2a_and_background_parse() {
        let file = parse(
            r#"
timezone = "UTC"

[gateway]
bind = "0.0.0.0"
port = 8080

[a2a]
enabled = false
port = 9999
public_url = "https://example.com/a2a/laptop"

[background]
max_concurrent = 10
hop_soft_limit = 4
hop_hard_limit = 16
"#,
        );
        let cfg = from_file_and_env(Some(&file), &test_dir()).unwrap();
        assert_eq!(cfg.gateway.bind, "0.0.0.0");
        assert_eq!(cfg.gateway.port, 8080);
        assert!(!cfg.a2a.enabled);
        assert_eq!(cfg.a2a.port, 9999);
        assert_eq!(
            cfg.a2a.public_url.as_deref(),
            Some("https://example.com/a2a/laptop")
        );
        assert_eq!(cfg.background.max_concurrent, 10);
        assert_eq!(cfg.background.hop_soft_limit, 4);
        assert_eq!(cfg.background.hop_hard_limit, 16);
    }

    #[test]
    fn push_contact_is_unset_by_default() {
        let file = parse("timezone = \"UTC\"\n");
        let cfg = from_file_and_env(Some(&file), &test_dir()).unwrap();
        assert_eq!(cfg.push, HubPushConfig::default());
        assert!(
            cfg.load_notices.is_empty(),
            "expected {:?} to be empty",
            cfg.load_notices
        );
    }

    #[test]
    fn push_contact_accepts_mailto_and_https() {
        for contact in ["mailto:bear@example.com", "https://example.com/contact"] {
            let file = parse(&format!(
                "timezone = \"UTC\"\n\n[push]\ncontact = \"{contact}\"\n"
            ));
            let cfg = from_file_and_env(Some(&file), &test_dir()).unwrap();
            assert_eq!(cfg.push.contact.as_deref(), Some(contact));
            assert!(cfg.load_notices.is_empty(), "{contact}");
        }
    }

    #[test]
    fn empty_push_contact_counts_as_unset() {
        let file = parse("timezone = \"UTC\"\n\n[push]\ncontact = \"  \"\n");
        let cfg = from_file_and_env(Some(&file), &test_dir()).unwrap();
        assert_eq!(cfg.push.contact, None);
        assert!(
            cfg.load_notices.is_empty(),
            "expected {:?} to be empty",
            cfg.load_notices
        );
    }

    #[test]
    fn invalid_push_contact_is_dropped_with_a_notice() {
        for contact in [
            "bear@example.com",
            "http://example.com",
            "mailto:",
            "mailto:no-at-sign",
            "mailto:bear@localhost",
            "mailto:a b@example.com",
            "https://",
            "ftp://example.com",
        ] {
            let file = parse(&format!(
                "timezone = \"UTC\"\n\n[push]\ncontact = \"{contact}\"\n"
            ));
            let cfg = from_file_and_env(Some(&file), &test_dir()).unwrap();
            assert_eq!(cfg.push.contact, None, "{contact}");
            assert!(
                cfg.load_notices
                    .iter()
                    .any(|n| n.contains("[push] contact") && n.contains(contact)),
                "{contact} should raise a notice naming it: {:?}",
                cfg.load_notices
            );
        }
    }

    #[test]
    fn cloud_token_env_override_still_applies() {
        let _guard = ENV_MUTEX.lock().unwrap();
        // SAFETY: test-only, serialized by ENV_MUTEX.
        unsafe { std::env::set_var("RESIDUUM_CLOUD_TOKEN", "env-cloud-token") };
        let cfg = from_file_and_env(Some(&parse("timezone = \"UTC\"\n\n[cloud]\n")), &test_dir());
        // SAFETY: test-only, serialized by ENV_MUTEX.
        unsafe { std::env::remove_var("RESIDUUM_CLOUD_TOKEN") };
        let cfg = cfg.unwrap();
        assert_eq!(
            cfg.cloud.as_ref().map(|c| c.token.as_str()),
            Some("env-cloud-token")
        );
    }

    #[test]
    fn a_cloud_section_with_the_retired_keys_still_loads_and_connects() {
        let _guard = ENV_MUTEX.lock().unwrap();
        for retired in [
            "remote_access = false",
            "remote_access = true",
            "local_port = 7700",
            "remote_access = false\nlocal_port = 9000",
        ] {
            let file = parse(&format!(
                "timezone = \"UTC\"\n\n[cloud]\ntoken = \"tok\"\n{retired}\n"
            ));
            let cfg = from_file_and_env(Some(&file), &test_dir()).unwrap();
            assert_eq!(
                cfg.cloud.as_ref().map(|c| c.token.as_str()),
                Some("tok"),
                "{retired}"
            );
        }
    }

    #[test]
    fn agent_only_sections_are_rejected() {
        for agent_only in [
            "[memory]\nobserver_cooldown_secs = 1\n",
            "[discord]\ntoken = \"x\"\n",
            "workspace_dir = \"/tmp/x\"\n",
            "name = \"sam\"\n",
            "[a2a]\nvisibility = \"private\"\n",
            "[background]\nsubagent_depth_cap = 2\n",
        ] {
            assert!(
                toml::from_str::<HubConfigFile>(agent_only).is_err(),
                "hub config should reject {agent_only:?}"
            );
        }
    }

    #[test]
    fn pricing_is_empty_by_default() {
        let file = parse("timezone = \"UTC\"\n");
        let cfg = from_file_and_env(Some(&file), &test_dir()).unwrap();
        assert!(cfg.pricing.is_empty());
    }

    #[test]
    fn pricing_table_is_keyed_by_provider_and_model() {
        let file = parse(
            r#"
timezone = "UTC"

[pricing."anthropic/claude-sonnet-4-6"]
input = 3.0
output = 15.0
cache_read = 0.3
cache_write = 3.75
cache_write_long = 6.0

[pricing."fireworks/accounts/fireworks/models/kimi-k3"]
input = 3.0
output = 15.0
cache_read = 0.3
cache_write = 3.0
cache_write_long = 3.0
"#,
        );
        let cfg = from_file_and_env(Some(&file), &test_dir()).unwrap();
        assert_eq!(cfg.pricing.len(), 2);
        assert_eq!(
            cfg.pricing.get("anthropic/claude-sonnet-4-6"),
            Some(&ModelPrice {
                input: 3.0,
                output: 15.0,
                cache_read: 0.3,
                cache_write: 3.75,
                cache_write_long: 6.0,
            })
        );
        assert!(
            cfg.pricing
                .contains_key("fireworks/accounts/fireworks/models/kimi-k3")
        );
    }

    #[test]
    fn a_negative_or_non_finite_price_is_dropped_with_a_notice() {
        for value in ["-2.5", "nan", "inf"] {
            let file = parse(&format!(
                "timezone = \"UTC\"\n\n[pricing.\"openai/gpt-4o\"]\ninput = {value}\noutput = 10.0\ncache_read = 1.25\ncache_write = 2.5\ncache_write_long = 2.5\n\n[pricing.\"anthropic/claude-sonnet-4-6\"]\n{FULL_PRICE}"
            ));
            let cfg = from_file_and_env(Some(&file), &test_dir()).unwrap();
            assert!(
                !cfg.pricing.contains_key("openai/gpt-4o"),
                "{value} is not a price"
            );
            assert!(
                cfg.pricing.contains_key("anthropic/claude-sonnet-4-6"),
                "a valid entry beside the bad one still loads"
            );
            assert!(
                cfg.load_notices
                    .iter()
                    .any(|n| n.contains("[pricing] \"openai/gpt-4o\" input")),
                "{value} should raise a notice naming the key and rate: {:?}",
                cfg.load_notices
            );
        }
    }

    #[test]
    fn a_price_key_must_name_a_known_provider_and_model() {
        for key in ["gpt-4o", "mistral/large", "openai/"] {
            let file = parse(&format!(
                "timezone = \"UTC\"\n\n[pricing.\"{key}\"]\n{FULL_PRICE}"
            ));
            let cfg = from_file_and_env(Some(&file), &test_dir()).unwrap();
            assert!(cfg.pricing.is_empty(), "{key} should not be priced");
            assert!(
                cfg.load_notices
                    .iter()
                    .any(|n| n.contains(&format!("[pricing] \"{key}\""))),
                "{key} should raise a notice naming it: {:?}",
                cfg.load_notices
            );
        }
    }

    #[test]
    fn a_price_entry_must_state_every_rate() {
        let result = toml::from_str::<HubConfigFile>(
            "[pricing.\"openai/gpt-4o\"]\ninput = 2.5\noutput = 10.0\n",
        );
        assert!(
            result.is_err(),
            "a price without cache rates should not load"
        );
    }
}
