//! Per-role completion options resolved from the agent's global settings.

use residuum_config::Config;

use super::CompletionOptions;

/// Build `CompletionOptions` for a named role, applying per-role overrides
/// over the global defaults.
#[must_use]
pub fn completion_options_for_role(cfg: &Config, role: &str) -> CompletionOptions {
    let ov = cfg.role_overrides.get(role);
    CompletionOptions {
        max_tokens: Some(cfg.max_tokens),
        temperature: ov.and_then(|o| o.temperature).or(cfg.temperature),
        thinking: ov.and_then(|o| o.thinking.clone()).or(cfg.thinking.clone()),
        ..CompletionOptions::default()
    }
}

#[cfg(test)]
mod tests {
    use residuum_config::testing::{test_agent_dir, test_hub_config};
    use residuum_config::{
        AgentConfigFile, ProvidersFile, ThinkingConfig, ThinkingLevel, from_file_and_env,
    };

    use super::completion_options_for_role;

    #[test]
    fn completion_options_for_role_with_override() {
        let prov_file: ProvidersFile = toml::from_str(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
observer = { model = "gemini/gemini-3.0-flash", temperature = 0.2 }
"#,
        )
        .unwrap();
        let cfg_file: AgentConfigFile = toml::from_str(
            "
temperature = 0.8
",
        )
        .unwrap();
        let cfg = from_file_and_env(
            Some(&cfg_file),
            Some(&prov_file),
            &test_agent_dir(),
            "test-agent",
            &test_hub_config(),
        )
        .unwrap();

        let main_opts = completion_options_for_role(&cfg, "main");
        assert_eq!(
            main_opts.temperature,
            Some(0.8),
            "main should use global temp"
        );

        let obs_opts = completion_options_for_role(&cfg, "observer");
        assert_eq!(
            obs_opts.temperature,
            Some(0.2),
            "observer should use override temp"
        );
    }

    #[test]
    fn completion_options_for_role_fallback_to_global() {
        let prov_file: ProvidersFile = toml::from_str(
            r#"
[models]
main = "anthropic/claude-sonnet-4-6"
"#,
        )
        .unwrap();
        let cfg_file: AgentConfigFile = toml::from_str(
            r#"
temperature = 0.5
thinking = "medium"
"#,
        )
        .unwrap();
        let cfg = from_file_and_env(
            Some(&cfg_file),
            Some(&prov_file),
            &test_agent_dir(),
            "test-agent",
            &test_hub_config(),
        )
        .unwrap();

        let opts = completion_options_for_role(&cfg, "reflector");
        assert_eq!(opts.temperature, Some(0.5), "should fall back to global");
        assert_eq!(
            opts.thinking,
            Some(ThinkingConfig::Level(ThinkingLevel::Medium)),
            "should fall back to global thinking"
        );
    }
}
