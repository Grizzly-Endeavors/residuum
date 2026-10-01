//! Config resolution logic: maps raw TOML structs + env vars into validated Config.

mod a2a;
mod agent;
mod background;
mod channels;
mod gateway;
pub(crate) mod hub;
mod memory;
mod models;
mod subconscious;
mod tracing_config;
mod web_search;

pub(crate) use hub::from_file_and_env as resolve_hub_config;

use std::path::Path;

use crate::inference::{ThinkingConfig, ThinkingLevel};
use crate::util::FatalError;

use super::Config;
use super::HubConfig;
use super::constants::{DEFAULT_MAX_TOKENS, DEFAULT_PULSE_ENABLED, DEFAULT_TIMEOUT_SECS};
use super::deserialize::{AgentConfigFile, ProvidersFile};
use super::secrets::SecretStore;

/// Load the secret store, degrading to an empty one (with a notice) if it
/// can't be read or decrypted.
///
/// A missing or corrupt secret store only matters to entries that actually
/// resolve a `secret:` reference — those already treat an unresolvable
/// reference as "missing" via `resolve_secret_value`'s `None`, and degrade
/// (or are skipped) through the same paths as any other missing required
/// value. Everything else in config.toml and providers.toml never touches
/// the store, so it must not fail the whole config load here.
///
/// The secret store is hub-level and shared by every agent, so `config_dir`
/// here is always the *hub's* directory, never an agent's own `config/`.
fn load_secrets_degraded(config_dir: &Path, notices: &mut Vec<String>) -> SecretStore {
    match SecretStore::load(config_dir) {
        Ok(store) => store,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "secret store degraded: secret: references will resolve as missing"
            );
            notices.push(format!(
                "Your secret store couldn't be loaded ({err}). Any setting using secret:<name> will be treated as missing until you fix or recreate it."
            ));
            SecretStore::empty()
        }
    }
}

/// Build an agent's `Config` from its own config file, its providers file,
/// and the hub config it belongs to.
///
/// `agent_dir` is the agent's workspace root (`~/.residuum/<name>`); the
/// agent's own `config.toml`/`providers.toml` live in `agent_dir/config/`.
/// Hub-owned values (timezone, gateway, cloud, tracing, the A2A listener's
/// enablement/port/public URL, and the shared background session budget and
/// hop limits) are copied in from `hub`; the agent's own file does not carry
/// those sections.
///
/// # Errors
/// Returns `FatalError::Config` if the model spec cannot be parsed, a
/// background model tier can't be resolved, or `agent.max_tool_iterations`
/// is set to `0`.
#[tracing::instrument(skip_all, fields(agent = %agent_name, agent_dir = %agent_dir.display()))]
pub(crate) fn from_file_and_env(
    file: Option<&AgentConfigFile>,
    providers_file: Option<&ProvidersFile>,
    agent_dir: &Path,
    agent_name: &str,
    hub: &HubConfig,
) -> Result<Config, FatalError> {
    let mut notices: Vec<String> = Vec::new();
    let secrets = load_secrets_degraded(&hub.config_dir, &mut notices);
    let providers_map = providers_file.and_then(|f| f.providers.as_ref());
    let models_section = providers_file.and_then(|f| f.models.as_ref());

    let mut resolved_models =
        models::resolve_all_model_specs(models_section, providers_map, &secrets)?;

    let workspace_dir = agent_dir.to_path_buf();
    let agent_config_dir = workspace_dir.join("config");

    let timeout_secs = file
        .and_then(|f| f.timeout_secs)
        .unwrap_or(DEFAULT_TIMEOUT_SECS);
    let max_tokens = file
        .and_then(|f| f.max_tokens)
        .unwrap_or(DEFAULT_MAX_TOKENS);
    let memory = memory::resolve_memory_config(file.and_then(|f| f.memory.as_ref()));
    let pulse_enabled = file
        .and_then(|f| f.pulse.as_ref())
        .and_then(|p| p.enabled)
        .unwrap_or(DEFAULT_PULSE_ENABLED);
    let subconscious_settings =
        subconscious::resolve_subconscious_settings(file.and_then(|f| f.subconscious.as_ref()));
    let learning = subconscious::resolve_learning_config(file.and_then(|f| f.learning.as_ref()));

    let (discord, telegram, teams, idle) =
        channels::resolve_configured_chats(file, &secrets, &mut notices);
    let a2a = a2a::resolve_agent_a2a_config(hub, file.and_then(|f| f.a2a.as_ref()), &mut notices);
    let webhooks = channels::resolve_webhooks_config(
        file.and_then(|f| f.webhooks.as_ref()),
        &secrets,
        &mut notices,
    );
    let skills = agent::resolve_skills_config(file.and_then(|f| f.skills.as_ref()), &workspace_dir);
    let tools = agent::resolve_tools_config(file.and_then(|f| f.tools.as_ref()), &hub.config_dir);
    let retry = agent::resolve_retry_config(file);

    let agent_abilities = agent::resolve_agent_config(file.and_then(|f| f.agent.as_ref()))?;

    let mut background = background::resolve_background_config(
        file.and_then(|f| f.background.as_ref()),
        providers_file
            .and_then(|pf| pf.background.as_ref())
            .and_then(|b| b.models.as_ref()),
        providers_map,
        &secrets,
        &mut resolved_models.role_overrides,
        hub,
    )?;

    models::scope_all_session_affinity(
        &mut resolved_models,
        &mut background.models,
        &workspace_dir,
    );

    let autostart = file.and_then(|f| f.autostart).unwrap_or(true);

    let thinking = file
        .and_then(|f| f.thinking.as_deref())
        .map(parse_thinking_config)
        .transpose()?;

    let web_search = web_search::resolve_web_search_config(
        file.and_then(|f| f.web_search.as_ref()),
        &resolved_models.main,
        &secrets,
    );

    Ok(Config {
        agent_name: agent_name.to_string(),
        autostart,
        main: resolved_models.main,
        observer: resolved_models.observer,
        reflector: resolved_models.reflector,
        pulse: resolved_models.pulse,
        subconscious: resolved_models.subconscious,
        embedding: resolved_models.embedding,
        workspace_dir,
        timeout_secs,
        max_tokens,
        memory,
        pulse_enabled,
        subconscious_settings,
        learning,
        gateway: hub.gateway.clone(),
        timezone: hub.timezone,
        cloud: hub.cloud.clone(),
        discord,
        telegram,
        teams,
        a2a,
        webhooks,
        skills,
        tools,
        retry,
        background,
        agent: agent_abilities,
        idle,
        temperature: file.and_then(|f| f.temperature),
        thinking,
        web_search,
        tracing: hub.tracing.clone(),
        role_overrides: resolved_models.role_overrides,
        config_dir: agent_config_dir,
        load_notices: notices,
    })
}

/// Expand `${ENV_VAR}` references in a token string.
///
/// Returns `Some(value)` if expansion succeeds or the string contains no `${...}`.
/// Returns `None` if the referenced env var is not set.
fn expand_env_token(raw: &str) -> Option<String> {
    match super::secrets::env_var_name(raw) {
        Some(var_name) => std::env::var(var_name).ok(),
        None => Some(raw.to_string()),
    }
}

/// Resolve a secret reference. Supports three modes:
/// - `${ENV_VAR}` → environment variable lookup
/// - `secret:name` → encrypted secrets file lookup
/// - Anything else → literal string passthrough
pub(super) fn resolve_secret_value(raw: &str, secrets: &SecretStore) -> Option<String> {
    if let Some(name) = raw.strip_prefix("secret:") {
        return secrets.get(name).map(String::from);
    }
    expand_env_token(raw)
}

/// Parse a thinking config string into a `ThinkingConfig`.
fn parse_thinking_config(value: &str) -> Result<ThinkingConfig, FatalError> {
    match value.to_lowercase().as_str() {
        "off" | "false" => Ok(ThinkingConfig::Toggle(false)),
        "on" | "true" => Ok(ThinkingConfig::Toggle(true)),
        "low" => Ok(ThinkingConfig::Level(ThinkingLevel::Low)),
        "medium" => Ok(ThinkingConfig::Level(ThinkingLevel::Medium)),
        "high" => Ok(ThinkingConfig::Level(ThinkingLevel::High)),
        other => Err(FatalError::Config(format!(
            "invalid thinking value '{other}': expected one of: off, on, low, medium, high"
        ))),
    }
}

#[cfg(test)]
pub(super) mod test_helpers {
    pub(super) use super::super::deserialize::{AgentConfigFile, ProvidersFile};
    pub(super) use super::super::secrets::SecretStore;

    pub(super) static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    pub(super) fn empty_secrets() -> SecretStore {
        let dir = std::env::temp_dir().join("residuum-test-empty-secrets");
        SecretStore::load(&dir).unwrap()
    }

    pub(super) fn test_agent_dir() -> std::path::PathBuf {
        std::env::temp_dir().join("residuum-test-agent")
    }

    pub(super) fn test_hub_config() -> super::super::HubConfig {
        super::super::HubConfig {
            timezone: chrono_tz::UTC,
            gateway: super::super::GatewayConfig::default(),
            cloud: None,
            a2a: super::super::HubA2aConfig::default(),
            tracing: super::super::TracingConfig::default(),
            background: super::super::HubBackgroundConfig::default(),
            config_dir: std::env::temp_dir().join("residuum-test-hub"),
            load_notices: Vec::new(),
        }
    }

    pub(super) fn parse_config(toml: &str) -> AgentConfigFile {
        toml::from_str(toml).unwrap()
    }

    pub(super) fn parse_providers(toml: &str) -> ProvidersFile {
        toml::from_str(toml).unwrap()
    }

    /// Resolve an agent `Config` from raw TOML sources against a default
    /// test hub config — the shape almost every resolve test needs.
    pub(super) fn resolve_test(
        cfg_toml: &str,
        providers_toml: &str,
    ) -> Result<super::super::Config, crate::util::FatalError> {
        resolve_test_with_hub(cfg_toml, providers_toml, &test_hub_config())
    }

    /// [`resolve_test`], with an explicit hub config for tests that need to
    /// vary hub-owned values.
    pub(super) fn resolve_test_with_hub(
        cfg_toml: &str,
        providers_toml: &str,
        hub: &super::super::HubConfig,
    ) -> Result<super::super::Config, crate::util::FatalError> {
        let cfg_file = parse_config(cfg_toml);
        let prov_file = parse_providers(providers_toml);
        super::from_file_and_env(
            Some(&cfg_file),
            Some(&prov_file),
            &test_agent_dir(),
            "test-agent",
            hub,
        )
    }
}

/// Agent-scoped environment variables the loader never reads. Each would
/// apply the same value to every agent in the process (e.g. one Discord token
/// for all), so an agent's values come only from its own
/// `config.toml`/`providers.toml`. One that is set in the environment gets a
/// notice at startup, see [`removed_agent_env_override_notices`].
const REMOVED_AGENT_ENV_OVERRIDES: &[&str] = &[
    "RESIDUUM_WORKSPACE",
    "RESIDUUM_MODEL",
    "RESIDUUM_OBSERVER_MODEL",
    "RESIDUUM_REFLECTOR_MODEL",
    "RESIDUUM_OBSERVER_API_KEY",
    "RESIDUUM_REFLECTOR_API_KEY",
    "RESIDUUM_PROVIDER_URL",
    "RESIDUUM_API_KEY",
    "RESIDUUM_DISCORD_TOKEN",
    "RESIDUUM_TELEGRAM_TOKEN",
    "RESIDUUM_TEAMS_APP_PASSWORD",
];

/// One notice (also logged at `warn`) per agent-scoped environment variable
/// from [`REMOVED_AGENT_ENV_OVERRIDES`] that is set in the process
/// environment. Called once at startup, not on config load, so a config
/// reload does not repeat it.
pub(crate) fn removed_agent_env_override_notices() -> Vec<String> {
    let mut notices = Vec::new();
    for var in REMOVED_AGENT_ENV_OVERRIDES {
        if std::env::var(var).is_ok() {
            tracing::warn!(
                %var,
                "environment variable is set but ignored; set the value in the agent's config.toml or providers.toml instead"
            );
            notices.push(format!(
                "The environment variable {var} is set, but Residuum ignores it. Put the value in the agent's config.toml or providers.toml instead."
            ));
        }
    }
    notices
}

#[cfg(test)]
#[expect(clippy::indexing_slicing, reason = "test assertions")]
#[expect(
    unsafe_code,
    reason = "std::env::set_var/remove_var require unsafe in edition 2024"
)]
mod tests {
    use std::path::PathBuf;

    use super::super::constants::{
        DEFAULT_DISCORD_CONTEXT_MESSAGES, DEFAULT_OBSERVER_COOLDOWN_SECS,
        DEFAULT_OBSERVER_FORCE_THRESHOLD, DEFAULT_OBSERVER_THRESHOLD, DEFAULT_REFLECTOR_THRESHOLD,
        DEFAULT_TEAMS_CONTEXT_MESSAGES, DEFAULT_TEAMS_PORT, DEFAULT_TELEGRAM_CONTEXT_MESSAGES,
    };
    use super::super::deserialize::{SearchConfigFile, ToolsConfigFile};
    use super::super::types::A2aVisibility;
    use super::test_helpers::*;
    use super::*;

    // ── Section-specific resolution ───────────────────────────────────────────

    #[test]
    fn resolve_tools_config_defaults_to_bin_dir() {
        let hub_dir = Path::new("/home/x/.residuum/hub");
        let tools = agent::resolve_tools_config(None, hub_dir);
        assert_eq!(
            tools.dirs,
            vec![PathBuf::from("/home/x/.residuum/hub/bin")],
            "with no section, only the default hub bin dir is present"
        );
    }

    #[test]
    fn resolve_tools_config_prepends_configured_dirs_before_bin() {
        let hub_dir = Path::new("/home/x/.residuum/hub");
        let section = ToolsConfigFile {
            path: Some(vec![
                "/opt/residuum-tools".to_string(),
                "~/extra-bin".to_string(),
            ]),
        };
        let tools = agent::resolve_tools_config(Some(&section), hub_dir);
        let expected_extra = PathBuf::from(shellexpand::tilde("~/extra-bin").as_ref());
        assert_eq!(
            tools.dirs,
            vec![
                PathBuf::from("/opt/residuum-tools"),
                expected_extra,
                PathBuf::from("/home/x/.residuum/hub/bin"),
            ],
            "configured dirs come first (in order), then the default hub bin dir"
        );
    }

    #[test]
    fn deny_unknown_fields_rejects_top_level_typos() {
        let toml_str = "
[memori]
observer_threshold_tokens = 30000
";
        let result = toml::from_str::<AgentConfigFile>(toml_str);
        assert!(
            result.is_err(),
            "unknown top-level section should be rejected"
        );
    }

    #[test]
    fn subconscious_defaults_to_disabled_when_absent() {
        let cfg = resolve_test("", "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n").unwrap();
        assert!(
            !cfg.subconscious_settings.enabled,
            "subconscious is opt-in and must default to disabled"
        );
        assert!(
            cfg.subconscious_settings.mid_turn,
            "mid_turn defaults to true (gated by enabled)"
        );
    }

    #[test]
    fn artifact_idle_timeout_defaults_to_ten_minutes_and_parses_from_background() {
        let providers = "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n";
        let defaults = resolve_test("", providers).unwrap();
        assert_eq!(
            defaults.background.idle_timeout_artifact,
            std::time::Duration::from_mins(10)
        );

        let cfg = resolve_test(
            "[background]\nidle_timeout_artifact_minutes = 25\n",
            providers,
        )
        .unwrap();
        assert_eq!(
            cfg.background.idle_timeout_artifact,
            std::time::Duration::from_mins(25)
        );
        assert_eq!(
            cfg.background.idle_timeout_spawned, defaults.background.idle_timeout_spawned,
            "the artifact setting must not bleed into another category's timeout"
        );
    }

    #[test]
    fn background_hub_limits_are_copied_in_not_settable_by_the_agent() {
        let mut hub = test_hub_config();
        hub.background.max_concurrent = 7;
        hub.background.hop_soft_limit = 2;
        hub.background.hop_hard_limit = 9;
        let cfg = resolve_test_with_hub(
            "",
            "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n",
            &hub,
        )
        .unwrap();
        assert_eq!(cfg.background.max_concurrent, 7);
        assert_eq!(cfg.background.hop_soft_limit, 2);
        assert_eq!(cfg.background.hop_hard_limit, 9);

        // The agent's own [background] section cannot set these: an attempt
        // to is an unknown-field parse error.
        assert!(toml::from_str::<AgentConfigFile>("[background]\nmax_concurrent = 99\n").is_err());
        assert!(toml::from_str::<AgentConfigFile>("[background]\nhop_soft_limit = 1\n").is_err());
    }

    #[test]
    fn hub_owned_fields_are_copied_from_hub_not_agent_file() {
        let mut hub = test_hub_config();
        hub.timezone = "America/New_York".parse().unwrap();
        hub.gateway.port = 9001;
        hub.tracing.log_level = super::super::types::LogLevel::Trace;
        let cfg = resolve_test_with_hub(
            "",
            "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n",
            &hub,
        )
        .unwrap();
        assert_eq!(cfg.timezone.name(), "America/New_York");
        assert_eq!(cfg.gateway.port, 9001);
        assert_eq!(cfg.tracing.log_level, super::super::types::LogLevel::Trace);

        // None of these are settable in the agent's own file.
        assert!(toml::from_str::<AgentConfigFile>("[gateway]\nport = 1\n").is_err());
        assert!(toml::from_str::<AgentConfigFile>("[cloud]\nenabled = true\n").is_err());
        assert!(toml::from_str::<AgentConfigFile>("[tracing]\nlog_level = \"info\"\n").is_err());
        assert!(toml::from_str::<AgentConfigFile>("timezone = \"UTC\"\n").is_err());
    }

    #[test]
    fn removed_agent_env_overrides_produce_a_startup_notice() {
        let _guard = ENV_MUTEX.lock().unwrap();
        // SAFETY: test-only, serialized by ENV_MUTEX.
        unsafe { std::env::set_var("RESIDUUM_MODEL", "openai/gpt-4o") };
        let notices = removed_agent_env_override_notices();
        unsafe { std::env::remove_var("RESIDUUM_MODEL") };
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(notices[0].contains("RESIDUUM_MODEL"));
        assert!(notices[0].contains("ignores it"));
    }

    #[test]
    fn loading_a_config_does_not_re_emit_removed_env_override_notices() {
        let _guard = ENV_MUTEX.lock().unwrap();
        // SAFETY: test-only, serialized by ENV_MUTEX.
        unsafe { std::env::set_var("RESIDUUM_MODEL", "openai/gpt-4o") };
        // Every reload goes through this same loader, so two loads stand in
        // for a startup followed by a reload.
        let first = resolve_test("", "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n").unwrap();
        let second =
            resolve_test("", "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n").unwrap();
        unsafe { std::env::remove_var("RESIDUUM_MODEL") };
        for cfg in [first, second] {
            assert!(
                cfg.load_notices
                    .iter()
                    .all(|n| !n.contains("RESIDUUM_MODEL")),
                "a config load must not carry the env override notice: {:?}",
                cfg.load_notices
            );
        }
    }

    #[test]
    fn removed_agent_env_overrides_have_no_effect_on_resolved_values() {
        let _guard = ENV_MUTEX.lock().unwrap();
        // SAFETY: test-only, serialized by ENV_MUTEX.
        unsafe {
            std::env::set_var("RESIDUUM_MODEL", "openai/gpt-4o");
            std::env::set_var("RESIDUUM_PROVIDER_URL", "http://override.invalid");
            std::env::set_var("RESIDUUM_API_KEY", "sk-env-override");
            std::env::set_var("RESIDUUM_DISCORD_TOKEN", "env-discord-token");
        }
        let cfg = resolve_test(
            "[discord]\n",
            "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n",
        );
        // SAFETY: test-only, serialized by ENV_MUTEX.
        unsafe {
            std::env::remove_var("RESIDUUM_MODEL");
            std::env::remove_var("RESIDUUM_PROVIDER_URL");
            std::env::remove_var("RESIDUUM_API_KEY");
            std::env::remove_var("RESIDUUM_DISCORD_TOKEN");
        }
        let cfg = cfg.unwrap();
        let main = cfg.main.first().unwrap();
        assert_eq!(main.model.model, "claude-sonnet-4-6");
        assert_ne!(main.provider_url, "http://override.invalid");
        assert_ne!(main.api_key.as_deref(), Some("sk-env-override"));
        assert!(
            cfg.discord.is_none(),
            "an env token must not enable Discord"
        );
    }

    #[test]
    fn no_removed_env_overrides_set_produces_no_extra_notice() {
        let _guard = ENV_MUTEX.lock().unwrap();
        for var in REMOVED_AGENT_ENV_OVERRIDES {
            // SAFETY: test-only, single-threaded test environment
            unsafe { std::env::remove_var(var) };
        }
        let cfg = resolve_test("", "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n").unwrap();
        assert!(cfg.load_notices.is_empty());
    }

    #[test]
    fn subconscious_knobs_parse() {
        let cfg_toml = "
[subconscious]
enabled = true
mid_turn = false
every_n_iterations = 5
max_transcript_tokens = 8000
learning = true
learning_cooldown_minutes = 60
";
        let cfg = resolve_test(
            cfg_toml,
            "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n",
        )
        .unwrap();
        assert!(cfg.subconscious_settings.enabled);
        assert!(!cfg.subconscious_settings.mid_turn);
        assert_eq!(cfg.subconscious_settings.every_n_iterations, 5);
        assert_eq!(cfg.subconscious_settings.max_transcript_tokens, 8000);
        assert!(cfg.subconscious_settings.learning, "learning parses");
        assert_eq!(cfg.subconscious_settings.learning_cooldown_minutes, 60);
    }

    #[test]
    fn learning_defaults_when_absent() {
        let cfg = resolve_test("", "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n").unwrap();
        assert!(
            !cfg.subconscious_settings.learning,
            "learning is opt-in, defaults to disabled"
        );
        assert_eq!(
            cfg.subconscious_settings.learning_cooldown_minutes, 240,
            "cooldown defaults to 240 minutes"
        );
        assert_eq!(
            cfg.learning.nudge_after_turns, 0,
            "fallback nudge defaults to disabled (0)"
        );
    }

    #[test]
    fn learning_nudge_fallback_parses() {
        let cfg = resolve_test(
            "[learning]\nnudge_after_turns = 12\n",
            "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n",
        )
        .unwrap();
        assert_eq!(cfg.learning.nudge_after_turns, 12);
    }

    #[test]
    fn subconscious_deny_unknown_fields() {
        let toml_str = "[subconscious]\nenalbed = true\n";
        let result = toml::from_str::<AgentConfigFile>(toml_str);
        assert!(
            result.is_err(),
            "typo in [subconscious] section should be rejected"
        );
    }

    #[test]
    fn memory_config_just_thresholds() {
        let cfg_toml = "
[memory]
observer_threshold_tokens = 20000
reflector_threshold_tokens = 50000
";
        let cfg = resolve_test(
            cfg_toml,
            "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n",
        )
        .unwrap();
        assert_eq!(cfg.memory.observer_threshold_tokens, 20000);
        assert_eq!(cfg.memory.reflector_threshold_tokens, 50000);
    }

    #[test]
    fn memory_config_defaults_when_absent() {
        let cfg = resolve_test("", "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n").unwrap();
        assert_eq!(
            cfg.memory.observer_threshold_tokens,
            DEFAULT_OBSERVER_THRESHOLD
        );
        assert_eq!(
            cfg.memory.reflector_threshold_tokens,
            DEFAULT_REFLECTOR_THRESHOLD
        );
    }

    #[test]
    fn pulse_enabled_defaults() {
        let cfg = resolve_test("", "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n").unwrap();
        assert!(cfg.pulse_enabled, "pulse should default to enabled");
    }

    #[test]
    fn discord_absent_returns_none() {
        let cfg = resolve_test("", "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n").unwrap();
        assert!(
            cfg.discord.is_none(),
            "no [discord] section should yield None"
        );
    }

    #[test]
    fn discord_section_without_token_returns_none() {
        let cfg = resolve_test(
            "[discord]\n",
            "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n",
        )
        .unwrap();
        assert!(
            cfg.discord.is_none(),
            "[discord] with no token should yield None"
        );
    }

    #[test]
    fn discord_section_with_token() {
        let cfg = resolve_test(
            "[discord]\ntoken = \"my-bot-token\"\n",
            "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n",
        )
        .unwrap();
        assert!(cfg.discord.is_some(), "[discord] with token should be Some");
        assert_eq!(
            cfg.discord.as_ref().map(|d| d.token.as_str()),
            Some("my-bot-token"),
            "token should match"
        );
        assert_eq!(
            cfg.discord.as_ref().map(|d| d.context_messages),
            Some(DEFAULT_DISCORD_CONTEXT_MESSAGES),
            "context_messages should default"
        );
    }

    #[test]
    fn discord_context_messages_override() {
        let cfg = resolve_test(
            "[discord]\ntoken = \"my-bot-token\"\ncontext_messages = 7\n",
            "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n",
        )
        .unwrap();
        assert_eq!(cfg.discord.as_ref().map(|d| d.context_messages), Some(7));
    }

    #[test]
    fn telegram_context_messages_defaults() {
        let cfg = resolve_test(
            "[telegram]\ntoken = \"tg-token\"\n",
            "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n",
        )
        .unwrap();
        assert_eq!(
            cfg.telegram.as_ref().map(|t| t.context_messages),
            Some(DEFAULT_TELEGRAM_CONTEXT_MESSAGES)
        );
    }

    #[test]
    fn telegram_context_messages_override() {
        let cfg = resolve_test(
            "[telegram]\ntoken = \"tg-token\"\ncontext_messages = 3\n",
            "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n",
        )
        .unwrap();
        assert_eq!(cfg.telegram.as_ref().map(|t| t.context_messages), Some(3));
    }

    #[test]
    fn webhooks_empty_when_absent() {
        let cfg = resolve_test("", "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n").unwrap();
        assert!(
            cfg.webhooks.is_empty(),
            "webhooks should be empty when absent"
        );
    }

    #[test]
    fn webhooks_single_entry() {
        let cfg_toml = r#"
[webhooks.github-issues]
secret = "my-secret"
routing = "agent:code_reviewer"
content_fields = ["issue.title", "issue.body"]
"#;
        let cfg = resolve_test(
            cfg_toml,
            "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n",
        )
        .unwrap();
        assert_eq!(cfg.webhooks.len(), 1);
        let entry = &cfg.webhooks["github-issues"];
        assert_eq!(entry.secret.as_deref(), Some("my-secret"));
        assert_eq!(
            entry.routing,
            crate::config::WebhookRouting::Agent("code_reviewer".to_string())
        );
        assert_eq!(entry.format, crate::config::WebhookFormat::Parsed);
        assert_eq!(
            entry.content_fields.as_deref(),
            Some(&["issue.title".to_string(), "issue.body".to_string()][..])
        );
    }

    #[test]
    fn webhooks_multiple_entries() {
        let cfg_toml = r#"
[webhooks.github]
secret = "gh-secret"
routing = "inbox"

[webhooks.deploy]
format = "raw"
"#;
        let cfg = resolve_test(
            cfg_toml,
            "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n",
        )
        .unwrap();
        assert_eq!(cfg.webhooks.len(), 2);

        let gh = &cfg.webhooks["github"];
        assert_eq!(gh.secret.as_deref(), Some("gh-secret"));
        assert_eq!(gh.routing, crate::config::WebhookRouting::Inbox);

        let deploy = &cfg.webhooks["deploy"];
        assert!(deploy.secret.is_none());
        assert_eq!(deploy.format, crate::config::WebhookFormat::Raw);
        assert_eq!(deploy.routing, crate::config::WebhookRouting::Inbox);
    }

    #[test]
    fn webhooks_default_routing() {
        let cfg = resolve_test(
            "[webhooks.simple]\nsecret = \"tok\"\n",
            "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n",
        )
        .unwrap();
        let entry = &cfg.webhooks["simple"];
        assert_eq!(
            entry.routing,
            crate::config::WebhookRouting::Inbox,
            "routing should default to inbox"
        );
        assert_eq!(
            entry.format,
            crate::config::WebhookFormat::Parsed,
            "format should default to parsed"
        );
    }

    #[test]
    fn webhooks_invalid_routing_is_skipped_with_notice() {
        // Counts notices, so the env-override tests must not add extras.
        let _guard = ENV_MUTEX.lock().unwrap();
        let cfg_toml = r#"
[webhooks.bad]
routing = "nowhere"

[webhooks.good]
routing = "inbox"
"#;
        let cfg = resolve_test(
            cfg_toml,
            "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n",
        )
        .unwrap();
        assert!(
            !cfg.webhooks.contains_key("bad"),
            "invalid webhook should be dropped, not fail the whole config"
        );
        assert!(
            cfg.webhooks.contains_key("good"),
            "other webhooks should still load"
        );
        assert_eq!(cfg.load_notices.len(), 1);
        let notice = cfg.load_notices.first().unwrap();
        assert!(
            notice.contains("bad"),
            "notice should name the webhook: {notice}"
        );
    }

    #[test]
    fn webhooks_empty_content_field_is_skipped_with_notice() {
        // Counts notices, so the env-override tests must not add extras.
        let _guard = ENV_MUTEX.lock().unwrap();
        let cfg = resolve_test(
            "[webhooks.bad]\ncontent_fields = [\"valid\", \"\"]\n",
            "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n",
        )
        .unwrap();
        assert!(!cfg.webhooks.contains_key("bad"));
        assert_eq!(cfg.load_notices.len(), 1);
        let notice = cfg.load_notices.first().unwrap();
        assert!(
            notice.contains("content_fields"),
            "notice should mention content_fields: {notice}"
        );
    }

    #[test]
    fn memory_config_cooldown_defaults() {
        let cfg = resolve_test("", "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n").unwrap();
        assert_eq!(
            cfg.memory.observer_cooldown_secs, DEFAULT_OBSERVER_COOLDOWN_SECS,
            "cooldown should default"
        );
        assert_eq!(
            cfg.memory.observer_force_threshold_tokens, DEFAULT_OBSERVER_FORCE_THRESHOLD,
            "force threshold should default"
        );
    }

    #[test]
    fn memory_config_cooldown_custom() {
        let cfg_toml = "
[memory]
observer_cooldown_secs = 60
observer_force_threshold_tokens = 50000
";
        let cfg = resolve_test(
            cfg_toml,
            "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n",
        )
        .unwrap();
        assert_eq!(
            cfg.memory.observer_cooldown_secs, 60,
            "cooldown should be custom"
        );
        assert_eq!(
            cfg.memory.observer_force_threshold_tokens, 50000,
            "force threshold should be custom"
        );
    }

    #[test]
    fn pulse_can_be_disabled() {
        let cfg = resolve_test(
            "[pulse]\nenabled = false\n",
            "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n",
        )
        .unwrap();
        assert!(!cfg.pulse_enabled);
    }

    // ── Agent abilities ──────────────────────────────────────────────────────

    #[test]
    fn agent_abilities_default_to_true() {
        let cfg = resolve_test("", "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n").unwrap();
        assert!(cfg.agent.modify_mcp, "modify_mcp should default to true");
        assert!(
            cfg.agent.modify_channels,
            "modify_channels should default to true"
        );
    }

    #[test]
    fn agent_abilities_custom_values() {
        let cfg_toml = "[agent]\nmodify_mcp = false\nmodify_channels = false\n";
        let cfg = resolve_test(
            cfg_toml,
            "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n",
        )
        .unwrap();
        assert!(!cfg.agent.modify_mcp, "modify_mcp should be false");
        assert!(
            !cfg.agent.modify_channels,
            "modify_channels should be false"
        );
    }

    #[test]
    fn max_tool_iterations_defaults_to_unlimited() {
        let cfg = resolve_test("", "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n").unwrap();
        assert_eq!(
            cfg.agent.max_tool_iterations, None,
            "an unset limit should mean unlimited"
        );
    }

    #[test]
    fn max_tool_iterations_round_trips_a_configured_value() {
        let cfg = resolve_test(
            "[agent]\nmax_tool_iterations = 25\n",
            "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n",
        )
        .unwrap();
        assert_eq!(cfg.agent.max_tool_iterations, Some(25));
    }

    #[test]
    fn max_tool_iterations_of_zero_is_rejected() {
        let err = resolve_test(
            "[agent]\nmax_tool_iterations = 0\n",
            "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n",
        )
        .expect_err("a zero limit should be rejected at load");
        assert!(
            err.to_string().contains("max_tool_iterations"),
            "error should name the offending setting: {err}"
        );
    }

    #[test]
    fn repeat_call_guard_defaults_to_three_and_six_enabled() {
        let cfg = resolve_test("", "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n").unwrap();
        assert!(
            cfg.agent.repeat_call_guard.enabled,
            "guard is on by default"
        );
        assert_eq!(cfg.agent.repeat_call_guard.steer_after, 3);
        assert_eq!(cfg.agent.repeat_call_guard.stop_after, 6);
    }

    #[test]
    fn repeat_call_guard_thresholds_and_disabling_are_configurable() {
        let cfg = resolve_test(
            "[agent]\nrepeat_call_steer_after = 2\nrepeat_call_stop_after = 4\n",
            "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n",
        )
        .unwrap();
        assert_eq!(cfg.agent.repeat_call_guard.steer_after, 2);
        assert_eq!(cfg.agent.repeat_call_guard.stop_after, 4);
        assert!(
            cfg.agent.repeat_call_guard.enabled,
            "still enabled by default"
        );

        let disabled_cfg = resolve_test(
            "[agent]\nrepeat_call_guard_enabled = false\n",
            "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n",
        )
        .unwrap();
        assert!(
            !disabled_cfg.agent.repeat_call_guard.enabled,
            "should be disableable"
        );
    }

    // ── Search config ─────────────────────────────────────────────────────

    #[test]
    fn search_config_defaults_when_absent() {
        let cfg = resolve_test("", "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n").unwrap();
        let search = &cfg.memory.search;
        assert!(
            (search.vector_weight - 0.7).abs() < f64::EPSILON,
            "vector_weight should default to 0.7"
        );
        assert!(
            (search.text_weight - 0.3).abs() < f64::EPSILON,
            "text_weight should default to 0.3"
        );
        assert!(
            (search.min_score - 0.35).abs() < f64::EPSILON,
            "min_score should default to 0.35"
        );
        assert_eq!(
            search.candidate_multiplier, 4,
            "candidate_multiplier should default to 4"
        );
    }

    #[test]
    fn search_config_custom_values() {
        let cfg_toml = "
[memory.search]
vector_weight = 0.5
text_weight = 0.5
min_score = 0.2
candidate_multiplier = 8
";
        let cfg = resolve_test(
            cfg_toml,
            "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n",
        )
        .unwrap();
        let search = &cfg.memory.search;
        assert!(
            (search.vector_weight - 0.5).abs() < f64::EPSILON,
            "vector_weight should be custom"
        );
        assert!(
            (search.text_weight - 0.5).abs() < f64::EPSILON,
            "text_weight should be custom"
        );
        assert!(
            (search.min_score - 0.2).abs() < f64::EPSILON,
            "min_score should be custom"
        );
        assert_eq!(
            search.candidate_multiplier, 8,
            "candidate_multiplier should be custom"
        );
    }

    fn assert_search_weights(toml_src: &str, expected_vector: f64, expected_text: f64) {
        let section: SearchConfigFile = toml::from_str(toml_src).unwrap();
        let cfg = memory::resolve_search_config(Some(&section));
        assert!(
            (cfg.vector_weight - expected_vector).abs() < 1e-9
                && (cfg.text_weight - expected_text).abs() < 1e-9,
            "{toml_src:?}: expected ({expected_vector}, {expected_text}), got ({}, {})",
            cfg.vector_weight,
            cfg.text_weight
        );
    }

    #[test]
    fn search_weights_normalize_to_sum_one() {
        assert_search_weights("vector_weight = 0.9\ntext_weight = 0.9\n", 0.5, 0.5);
        assert_search_weights("vector_weight = 3.0\ntext_weight = 1.0\n", 0.75, 0.25);
    }

    #[test]
    fn search_weights_single_override_keeps_other_default() {
        // text_weight stays at its 0.3 default, so 0.3 / (0.3 + 0.3)
        assert_search_weights("vector_weight = 0.3\n", 0.5, 0.5);
    }

    #[test]
    fn search_weight_zero_disables_that_signal() {
        assert_search_weights("vector_weight = 0.0\ntext_weight = 0.4\n", 0.0, 1.0);
    }

    #[test]
    fn search_weights_both_zero_fall_back_to_defaults() {
        assert_search_weights("vector_weight = 0.0\ntext_weight = 0.0\n", 0.7, 0.3);
    }

    #[test]
    fn search_weight_negative_or_non_finite_uses_default() {
        assert_search_weights("vector_weight = -1.0\ntext_weight = 0.3\n", 0.7, 0.3);
        assert_search_weights("vector_weight = 0.7\ntext_weight = nan\n", 0.7, 0.3);
        assert_search_weights("vector_weight = inf\ntext_weight = 0.3\n", 0.7, 0.3);
    }

    #[test]
    fn search_config_deny_unknown_fields() {
        let toml_str = "[memory.search]\ntypo_field = 0.5\n";
        let result = toml::from_str::<AgentConfigFile>(toml_str);
        assert!(
            result.is_err(),
            "unknown field in [memory.search] should be rejected"
        );
    }

    // ── Secret / env expansion ──────────────────────────────────────────────

    #[test]
    fn expand_env_token_literal() {
        assert_eq!(
            expand_env_token("plain-string"),
            Some("plain-string".to_string()),
            "literal should pass through"
        );
    }

    #[test]
    fn expand_env_token_present() {
        let _guard = ENV_MUTEX.lock().unwrap();
        // SAFETY: test-only, single-threaded test environment
        unsafe { std::env::set_var("RESIDUUM_TEST_SECRET_PRESENT", "found-it") };
        let result = expand_env_token("${RESIDUUM_TEST_SECRET_PRESENT}");
        assert_eq!(
            result,
            Some("found-it".to_string()),
            "should resolve env var"
        );
        unsafe { std::env::remove_var("RESIDUUM_TEST_SECRET_PRESENT") };
    }

    #[test]
    fn expand_env_token_missing() {
        // SAFETY: test-only, single-threaded test environment
        unsafe { std::env::remove_var("RESIDUUM_TEST_SECRET_MISSING") };
        let result = expand_env_token("${RESIDUUM_TEST_SECRET_MISSING}");
        assert!(result.is_none(), "missing env var should return None");
    }

    #[test]
    fn resolve_secret_value_env() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let secrets = empty_secrets();
        // SAFETY: test-only, single-threaded test environment
        unsafe { std::env::set_var("RESIDUUM_TEST_RSV_ENV", "env-val") };
        let result = resolve_secret_value("${RESIDUUM_TEST_RSV_ENV}", &secrets);
        assert_eq!(
            result,
            Some("env-val".to_string()),
            "should dispatch to env expansion"
        );
        unsafe { std::env::remove_var("RESIDUUM_TEST_RSV_ENV") };
    }

    #[test]
    fn resolve_secret_value_secret_store() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = SecretStore::load(dir.path()).unwrap();
        store.set("test_key", "secret-val", dir.path()).unwrap();

        let result = resolve_secret_value("secret:test_key", &store);
        assert_eq!(
            result,
            Some("secret-val".to_string()),
            "should dispatch to secret store"
        );
    }

    #[test]
    fn resolve_secret_value_literal() {
        let secrets = empty_secrets();
        let result = resolve_secret_value("plain-api-key", &secrets);
        assert_eq!(
            result,
            Some("plain-api-key".to_string()),
            "literal should pass through"
        );
    }

    // ── Gateway config (hub-level resolution, tested directly) ────────────────

    #[test]
    fn gateway_config_defaults_and_env_override() {
        let _guard = ENV_MUTEX.lock().unwrap();
        // Combined into one test to avoid env var races across parallel tests.
        // SAFETY: test-only environment
        unsafe {
            std::env::remove_var("RESIDUUM_GATEWAY_BIND");
            std::env::remove_var("RESIDUUM_GATEWAY_PORT");
        }

        // Defaults
        let cfg = gateway::resolve_gateway_config(None);
        assert_eq!(cfg.bind, "127.0.0.1", "default bind should be loopback");
        assert_eq!(cfg.port, 7700, "default port should be 7700");
        assert_eq!(cfg.addr(), "127.0.0.1:7700");

        // Env overrides
        unsafe {
            std::env::set_var("RESIDUUM_GATEWAY_BIND", "0.0.0.0");
            std::env::set_var("RESIDUUM_GATEWAY_PORT", "8080");
        }
        let env_cfg = gateway::resolve_gateway_config(None);
        assert_eq!(env_cfg.bind, "0.0.0.0", "env should override bind");
        assert_eq!(env_cfg.port, 8080, "env should override port");
        assert_eq!(env_cfg.addr(), "0.0.0.0:8080");
        unsafe {
            std::env::remove_var("RESIDUUM_GATEWAY_BIND");
            std::env::remove_var("RESIDUUM_GATEWAY_PORT");
        }
    }

    #[test]
    fn telegram_absent_returns_none() {
        let cfg = resolve_test("", "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n").unwrap();
        assert!(
            cfg.telegram.is_none(),
            "no [telegram] section should yield None"
        );
    }

    #[test]
    fn telegram_section_without_token_returns_none() {
        let cfg = resolve_test(
            "[telegram]\n",
            "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n",
        )
        .unwrap();
        assert!(
            cfg.telegram.is_none(),
            "[telegram] with no token should yield None"
        );
    }

    #[test]
    fn telegram_section_with_token() {
        let cfg = resolve_test(
            "[telegram]\ntoken = \"123456789:ABCdefGHIjklmnop\"\n",
            "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n",
        )
        .unwrap();
        assert!(
            cfg.telegram.is_some(),
            "[telegram] with token should be Some"
        );
        assert_eq!(
            cfg.telegram.as_ref().map(|t| t.token.as_str()),
            Some("123456789:ABCdefGHIjklmnop"),
            "token should match"
        );
    }

    fn chat_bots_config(extra: &str) -> Config {
        let cfg_toml = format!(
            "[discord]\ntoken = \"d-token\"\n{extra}\n[telegram]\ntoken = \"t-token\"\n{extra}"
        );
        resolve_test(
            &cfg_toml,
            "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n",
        )
        .unwrap()
    }

    #[test]
    fn chat_bots_are_owner_only_by_default() {
        let cfg = chat_bots_config("");
        assert_eq!(cfg.discord.map(|d| d.respond_to_others), Some(false));
        assert_eq!(cfg.telegram.map(|t| t.respond_to_others), Some(false));
    }

    #[test]
    fn chat_bots_respond_to_others_when_enabled() {
        let cfg = chat_bots_config("respond_to_others = true\n");
        assert_eq!(cfg.discord.map(|d| d.respond_to_others), Some(true));
        assert_eq!(cfg.telegram.map(|t| t.respond_to_others), Some(true));
    }

    // ── Teams config ───────────────────────────────────────────────────────

    #[test]
    fn teams_section_resolves_with_defaults() {
        let cfg_toml = r#"
[teams]
app_id = "11111111-2222-3333-4444-555555555555"
tenant_id = "tenant-guid"
app_password = "client-secret"
"#;
        let cfg = resolve_test(
            cfg_toml,
            "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n",
        )
        .unwrap();
        let teams = cfg.teams.unwrap();
        assert_eq!(teams.app_id, "11111111-2222-3333-4444-555555555555");
        assert_eq!(teams.tenant_id, "tenant-guid");
        assert_eq!(teams.app_password, "client-secret");
        assert!(!teams.respond_to_others, "owner-only by default");
        assert_eq!(teams.context_messages, DEFAULT_TEAMS_CONTEXT_MESSAGES);
        assert_eq!(teams.port, DEFAULT_TEAMS_PORT);
    }

    #[test]
    fn teams_section_honours_overrides() {
        let cfg_toml = r#"
[teams]
app_id = "app"
tenant_id = "tenant"
app_password = "secret"
respond_to_others = true
context_messages = 5
port = 8801
"#;
        let cfg = resolve_test(
            cfg_toml,
            "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n",
        )
        .unwrap();
        let teams = cfg.teams.unwrap();
        assert!(teams.respond_to_others);
        assert_eq!(teams.context_messages, 5);
        assert_eq!(teams.port, 8801);
    }

    #[test]
    fn teams_section_missing_required_field_disables_teams_with_notice() {
        // Counts notices, so the env-override tests must not add extras.
        let _guard = ENV_MUTEX.lock().unwrap();
        for (missing, toml) in [
            (
                "app_id",
                "[teams]\ntenant_id = \"t\"\napp_password = \"s\"\n",
            ),
            (
                "tenant_id",
                "[teams]\napp_id = \"a\"\napp_password = \"s\"\n",
            ),
            (
                "app_password",
                "[teams]\napp_id = \"a\"\ntenant_id = \"t\"\n",
            ),
        ] {
            let cfg =
                resolve_test(toml, "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n").unwrap();
            assert!(
                cfg.teams.is_none(),
                "teams should be disabled, not fail the config, when {missing} is missing"
            );
            assert_eq!(cfg.load_notices.len(), 1);
            let notice = cfg.load_notices.first().unwrap();
            assert!(
                notice.contains(missing),
                "notice should name {missing}: {notice}"
            );
        }
    }

    #[test]
    fn teams_absent_is_none_and_teams_is_a_valid_idle_channel() {
        assert!(
            resolve_test("", "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n")
                .unwrap()
                .teams
                .is_none()
        );

        let cfg_toml = r#"
[idle]
idle_channel = "teams"

[teams]
app_id = "a"
tenant_id = "t"
app_password = "s"
"#;
        let cfg = resolve_test(
            cfg_toml,
            "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n",
        )
        .unwrap();
        assert_eq!(cfg.idle.idle_channel.as_deref(), Some("teams"));
    }

    // ── A2A config: enabled/port/public_url are hub-owned, visibility is agent-owned ──

    #[test]
    fn a2a_absent_resolves_to_hub_defaults_and_public_visibility() {
        let cfg = resolve_test("", "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n").unwrap();
        assert!(cfg.a2a.enabled, "enabled by default (from hub)");
        assert_eq!(cfg.a2a.port, crate::config::DEFAULT_A2A_PORT);
        assert_eq!(cfg.a2a.public_url, None);
        assert_eq!(cfg.a2a.visibility, A2aVisibility::Public);
    }

    #[test]
    fn a2a_enabled_port_and_public_url_come_from_hub_not_the_agent_file() {
        let mut hub = test_hub_config();
        hub.a2a.enabled = false;
        hub.a2a.port = 9999;
        hub.a2a.public_url = Some("https://example.com/a2a/laptop".to_string());
        let cfg = resolve_test_with_hub(
            "",
            "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n",
            &hub,
        )
        .unwrap();
        assert!(!cfg.a2a.enabled);
        assert_eq!(cfg.a2a.port, 9999);
        assert_eq!(
            cfg.a2a.public_url.as_deref(),
            Some("https://example.com/a2a/laptop")
        );

        // The agent's own [a2a] section cannot set these.
        assert!(toml::from_str::<AgentConfigFile>("[a2a]\nenabled = false\n").is_err());
        assert!(toml::from_str::<AgentConfigFile>("[a2a]\nport = 1\n").is_err());
    }

    #[test]
    fn a2a_visibility_is_agent_owned() {
        let cfg = resolve_test(
            "[a2a]\nvisibility = \"private\"\n",
            "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n",
        )
        .unwrap();
        assert_eq!(cfg.a2a.visibility, A2aVisibility::Private);
    }

    #[test]
    fn a2a_invalid_visibility_falls_back_to_default_with_notice() {
        // Counts notices, so the env-override tests must not add extras.
        let _guard = ENV_MUTEX.lock().unwrap();
        let cfg = resolve_test(
            "[a2a]\nvisibility = \"hidden\"\n",
            "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n",
        )
        .unwrap();
        assert_eq!(
            cfg.a2a.visibility,
            A2aVisibility::default(),
            "a2a should still come up on the default visibility"
        );
        assert_eq!(cfg.load_notices.len(), 1);
        let notice = cfg.load_notices.first().unwrap();
        assert!(
            notice.contains("visibility"),
            "notice should mention visibility: {notice}"
        );
    }

    // ── Web search config ────────────────────────────────────────────────

    #[test]
    fn web_search_native_enabled_for_anthropic() {
        let cfg = resolve_test("", "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n").unwrap();
        assert!(
            cfg.web_search.provider_native.is_some(),
            "anthropic should get provider-native search"
        );
    }

    #[test]
    fn web_search_native_enabled_for_openai() {
        let cfg = resolve_test("", "[models]\nmain = \"openai/gpt-4o\"\n").unwrap();
        assert!(
            cfg.web_search.provider_native.is_some(),
            "openai should get provider-native search"
        );
    }

    #[test]
    fn web_search_native_enabled_for_gemini() {
        let cfg = resolve_test("", "[models]\nmain = \"gemini/gemini-2.0-flash\"\n").unwrap();
        assert!(
            cfg.web_search.provider_native.is_some(),
            "gemini should get provider-native search"
        );
    }

    #[test]
    fn web_search_native_disabled_for_ollama() {
        let cfg = resolve_test("", "[models]\nmain = \"ollama/llama3\"\n").unwrap();
        assert!(
            cfg.web_search.provider_native.is_none(),
            "ollama should not get provider-native search"
        );
    }

    #[test]
    fn web_search_native_disabled_for_fireworks() {
        let cfg = resolve_test(
            "",
            "[models]\nmain = \"fireworks/accounts/fireworks/routers/glm-flash-latest\"\n",
        )
        .unwrap();
        assert!(
            cfg.web_search.provider_native.is_none(),
            "fireworks has no hosted search tool"
        );
    }

    #[test]
    fn web_search_native_disabled_for_self_hosted_openai_compatible() {
        let providers = r#"
[providers.local-vllm]
type = "openai"
url = "http://localhost:8000/v1"

[models]
main = "local-vllm/qwen3"
"#;
        let cfg = resolve_test("", providers).unwrap();
        assert!(
            cfg.web_search.provider_native.is_none(),
            "compatible servers reject OpenAI's hosted search tool"
        );
    }

    #[test]
    fn web_search_native_enabled_for_openai_url_with_trailing_slash() {
        let providers = r#"
[providers.oai]
type = "openai"
url = "https://api.openai.com/v1/"

[models]
main = "oai/gpt-4o"
"#;
        let cfg = resolve_test("", providers).unwrap();
        assert!(
            cfg.web_search.provider_native.is_some(),
            "the OpenAI API itself still gets hosted search"
        );
    }

    // ── Session affinity ─────────────────────────────────────────────────

    #[test]
    fn session_affinity_is_scoped_per_role_and_stable() {
        let providers =
            "[models]\nmain = \"fireworks/accounts/fireworks/routers/glm-flash-latest\"\n";
        let first = resolve_test("", providers).unwrap();
        let second = resolve_test("", providers).unwrap();

        let main_key = first
            .main
            .first()
            .unwrap()
            .session_affinity
            .clone()
            .unwrap();
        let observer_key = first
            .observer
            .first()
            .unwrap()
            .session_affinity
            .clone()
            .unwrap();
        assert!(
            main_key.starts_with("residuum-main-"),
            "key names its role: {main_key}"
        );
        assert_ne!(
            main_key, observer_key,
            "roles inheriting main's chain still get their own key"
        );
        assert_eq!(
            first.main.first().unwrap().session_affinity,
            second.main.first().unwrap().session_affinity,
            "key must survive a config reload to keep the replica warm"
        );
        let workspace = first.workspace_dir.to_string_lossy().into_owned();
        assert!(
            !main_key.contains(&workspace),
            "workspace path must not leave the machine"
        );
    }

    #[test]
    fn web_search_anthropic_overrides() {
        let cfg_toml = r#"
[web_search.anthropic]
max_uses = 3
allowed_domains = ["example.com"]
blocked_domains = ["spam.com"]
"#;
        let cfg = resolve_test(
            cfg_toml,
            "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n",
        )
        .unwrap();
        let native = cfg.web_search.provider_native.as_ref().unwrap();
        assert_eq!(native.max_uses, Some(3), "max_uses should be set");
        assert_eq!(
            native.allowed_domains.as_deref(),
            Some(&["example.com".to_string()][..]),
            "allowed_domains should be set"
        );
        assert_eq!(
            native.blocked_domains.as_deref(),
            Some(&["spam.com".to_string()][..]),
            "blocked_domains should be set"
        );
    }

    #[test]
    fn web_search_openai_overrides() {
        let cfg = resolve_test(
            "[web_search.openai]\nsearch_context_size = \"high\"\n",
            "[models]\nmain = \"openai/gpt-4o\"\n",
        )
        .unwrap();
        let native = cfg.web_search.provider_native.as_ref().unwrap();
        assert_eq!(
            native.search_context_size.as_deref(),
            Some("high"),
            "search_context_size should be set"
        );
    }

    #[test]
    fn web_search_standalone_brave_with_literal_key() {
        let cfg_toml = r#"
[web_search]
backend = "brave"

[web_search.brave]
api_key = "BSA-test-key"
"#;
        let cfg = resolve_test(cfg_toml, "[models]\nmain = \"ollama/llama3\"\n").unwrap();
        let backend = cfg.web_search.standalone_backend.as_ref().unwrap();
        assert_eq!(backend.name, "brave", "backend name should be brave");
        assert_eq!(
            backend.api_key, "BSA-test-key",
            "api key should be resolved"
        );
    }

    #[test]
    fn web_search_standalone_no_key_warns() {
        let cfg_toml = "[web_search]\nbackend = \"brave\"\n\n[web_search.brave]\n";
        let cfg = resolve_test(cfg_toml, "[models]\nmain = \"ollama/llama3\"\n").unwrap();
        assert!(
            cfg.web_search.standalone_backend.is_none(),
            "backend without api key should be None"
        );
    }

    #[test]
    fn web_search_both_native_and_standalone_coexist() {
        let cfg_toml = r#"
[web_search]
backend = "brave"

[web_search.brave]
api_key = "BSA-test"

[web_search.anthropic]
max_uses = 5
"#;
        let cfg = resolve_test(
            cfg_toml,
            "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n",
        )
        .unwrap();
        assert!(
            cfg.web_search.provider_native.is_some(),
            "provider-native should be set"
        );
        assert!(
            cfg.web_search.standalone_backend.is_some(),
            "standalone backend should also be set"
        );
    }

    #[test]
    fn web_search_deny_unknown_fields() {
        let toml_str = "[web_search]\ntypo = \"bad\"\n";
        let result = toml::from_str::<AgentConfigFile>(toml_str);
        assert!(
            result.is_err(),
            "unknown field in [web_search] should be rejected"
        );
    }

    #[test]
    fn web_search_defaults_when_absent() {
        let cfg = resolve_test("", "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n").unwrap();
        assert!(
            cfg.web_search.standalone_backend.is_none(),
            "standalone should be None by default"
        );
        // provider_native is auto-set for anthropic
        assert!(
            cfg.web_search.provider_native.is_some(),
            "native should be auto-set for anthropic"
        );
        let native = cfg.web_search.provider_native.as_ref().unwrap();
        assert!(native.max_uses.is_none(), "max_uses should default to None");
    }

    #[test]
    fn autostart_defaults_to_true() {
        let cfg = resolve_test("", "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n").unwrap();
        assert!(cfg.autostart, "autostart should default to true");
    }

    #[test]
    fn autostart_can_be_disabled() {
        let cfg = resolve_test(
            "autostart = false\n",
            "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n",
        )
        .unwrap();
        assert!(!cfg.autostart);
    }

    #[test]
    fn agent_name_is_carried_through() {
        let cfg = resolve_test("", "[models]\nmain = \"anthropic/claude-sonnet-4-6\"\n").unwrap();
        assert_eq!(cfg.agent_name, "test-agent");
    }
}
