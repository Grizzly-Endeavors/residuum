//! Builds the `client` context block attached to every bug-report and
//! feedback submission.
//!
//! This module is the only place that decides what runtime metadata the
//! agent shares with the developer endpoint. Everything excluded here —
//! chat history, memory, file contents, API keys, file paths, URLs, names — is
//! intentionally out of scope.

use std::collections::BTreeMap;

use crate::config::Config;
use crate::inference::{ThinkingConfig, ThinkingLevel};

use super::{ClientContext, FeedbackClient};

/// Gather the static part of the client context for a bug report.
///
/// Reads version/commit from build-time env vars, OS/arch from
/// `std::env::consts`, and the active model from the resolved config.
///
/// `active_subagents` starts empty here: it's a live read of the session
/// registry, overlaid onto this snapshot by the bug-report tool and HTTP
/// handler at submission time, not something this startup-time snapshot can
/// know.
#[must_use]
pub fn gather_for_bug_report(config: &Config) -> ClientContext {
    let (model_provider, model_name) = config.main.first().map_or((None, None), |spec| {
        (
            Some(spec.model.kind.to_string()),
            Some(spec.model.model.clone()),
        )
    });

    ClientContext {
        version: env!("RESIDUUM_VERSION").to_string(),
        commit: option_env!("RESIDUUM_GIT_COMMIT").map(str::to_string),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        model_provider,
        model_name,
        active_subagents: Vec::new(),
        config_flags: config_flags(config),
    }
}

/// The allowlisted configuration values attached to a bug report.
///
/// Only booleans, counts, and fixed enums are listed, so nothing the user
/// typed can reach the report: no API keys, file paths, URLs, model or
/// channel names, or other free text. A new entry must keep to those kinds.
fn config_flags(config: &Config) -> BTreeMap<String, String> {
    let thinking = match &config.thinking {
        None => "unset".to_string(),
        Some(ThinkingConfig::Toggle(on)) => on.to_string(),
        Some(ThinkingConfig::Level(ThinkingLevel::Low)) => "low".to_string(),
        Some(ThinkingConfig::Level(ThinkingLevel::Medium)) => "medium".to_string(),
        Some(ThinkingConfig::Level(ThinkingLevel::High)) => "high".to_string(),
    };
    let entries: [(&str, String); 19] = [
        ("tracing.log_level", config.tracing.log_level.to_string()),
        (
            "tracing.auto_error_reporting",
            config.tracing.auto_error_reporting.to_string(),
        ),
        (
            "tracing.sanitize_content",
            config.tracing.sanitize_content.to_string(),
        ),
        (
            "tracing.otel_endpoint_count",
            config.tracing.otel_endpoints.len().to_string(),
        ),
        ("pulse.enabled", config.pulse_enabled.to_string()),
        (
            "subconscious.enabled",
            config.subconscious_settings.enabled.to_string(),
        ),
        (
            "subconscious.mid_turn",
            config.subconscious_settings.mid_turn.to_string(),
        ),
        (
            "subconscious.learning",
            config.subconscious_settings.learning.to_string(),
        ),
        ("agent.modify_mcp", config.agent.modify_mcp.to_string()),
        (
            "agent.modify_channels",
            config.agent.modify_channels.to_string(),
        ),
        (
            "memory.search.temporal_decay",
            config.memory.search.temporal_decay.to_string(),
        ),
        (
            "embedding.configured",
            config.embedding.is_some().to_string(),
        ),
        ("thinking", thinking),
        (
            "main.fallback_count",
            config.main.len().saturating_sub(1).to_string(),
        ),
        ("cloud.configured", config.cloud.is_some().to_string()),
        ("discord.configured", config.discord.is_some().to_string()),
        ("telegram.configured", config.telegram.is_some().to_string()),
        ("teams.configured", config.teams.is_some().to_string()),
        ("webhook_count", config.webhooks.len().to_string()),
    ];
    entries
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect()
}

/// Gather the (version-only) client context for a feedback submission.
///
/// The feedback wire contract accepts `client.version` only; this
/// helper exists so callers don't accidentally over-attach metadata.
#[must_use]
pub fn gather_for_feedback() -> FeedbackClient {
    FeedbackClient {
        version: env!("RESIDUUM_VERSION").to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRETS: [&str; 6] = [
        "sk-provider-secret",
        "discord-token-secret",
        "cloud-token-secret",
        "webhook-secret-value",
        "private-hook-name",
        "private-workspace-dir",
    ];

    fn load_config(config_toml: &str) -> Config {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), config_toml).unwrap();
        std::fs::write(
            dir.path().join("providers.toml"),
            "[providers.private-provider]\ntype = \"anthropic\"\napi_key = \"sk-provider-secret\"\n\n[models]\nmain = \"private-provider/claude-sonnet-4-6\"\n",
        )
        .unwrap();
        Config::load_at(dir.path()).unwrap()
    }

    fn full_config() -> Config {
        load_config(
            r#"
timezone = "UTC"
workspace_dir = "/home/someone/private-workspace-dir"

[discord]
token = "discord-token-secret"

[cloud]
token = "cloud-token-secret"

[webhooks.private-hook-name]
secret = "webhook-secret-value"

[tracing]
log_level = "trace"
auto_error_reporting = true
"#,
        )
    }

    #[test]
    fn config_flags_ship_exactly_the_allowlisted_keys() {
        let flags = config_flags(&full_config());
        let keys: Vec<&str> = flags.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            [
                "agent.modify_channels",
                "agent.modify_mcp",
                "cloud.configured",
                "discord.configured",
                "embedding.configured",
                "main.fallback_count",
                "memory.search.temporal_decay",
                "pulse.enabled",
                "subconscious.enabled",
                "subconscious.learning",
                "subconscious.mid_turn",
                "teams.configured",
                "telegram.configured",
                "thinking",
                "tracing.auto_error_reporting",
                "tracing.log_level",
                "tracing.otel_endpoint_count",
                "tracing.sanitize_content",
                "webhook_count",
            ]
        );
    }

    #[test]
    fn config_flags_reflect_config_values() {
        let flags = config_flags(&full_config());
        assert_eq!(
            flags.get("tracing.log_level").map(String::as_str),
            Some("trace")
        );
        assert_eq!(
            flags
                .get("tracing.auto_error_reporting")
                .map(String::as_str),
            Some("true")
        );
        assert_eq!(
            flags.get("discord.configured").map(String::as_str),
            Some("true")
        );
        assert_eq!(
            flags.get("telegram.configured").map(String::as_str),
            Some("false")
        );
        assert_eq!(
            flags.get("cloud.configured").map(String::as_str),
            Some("true")
        );
        assert_eq!(flags.get("webhook_count").map(String::as_str), Some("1"));
        assert_eq!(
            flags.get("main.fallback_count").map(String::as_str),
            Some("0")
        );
        assert_eq!(flags.get("thinking").map(String::as_str), Some("unset"));
    }

    #[test]
    fn config_flags_never_carry_secrets_names_or_paths() {
        let flags = config_flags(&full_config());
        for (key, value) in &flags {
            for secret in SECRETS {
                assert!(
                    !key.contains(secret) && !value.contains(secret),
                    "{key} = {value} leaks {secret}"
                );
            }
            assert!(
                !value.contains('/'),
                "{key} = {value} looks like a path or URL"
            );
            assert!(
                value.len() <= 8,
                "{key} = {value} is longer than any boolean, count, or enum"
            );
        }
    }
}
