//! Fixtures the gateway's tests share.

use residuum_config::Config;

/// A minimal agent config: no models, no chat interfaces, every section
/// at its default and UTC as the timezone.
pub(crate) fn test_config() -> Config {
    Config {
        agent_name: "test-agent".to_string(),
        autostart: true,
        main: vec![],
        observer: vec![],
        reflector: vec![],
        pulse: vec![],
        subconscious: vec![],
        embedding: None,
        workspace_dir: std::path::PathBuf::from("/tmp/test"),
        timeout_secs: 30,
        max_tokens: 4096,
        memory: residuum_config::MemoryConfig::default(),
        pulse_enabled: false,
        subconscious_settings: residuum_config::SubconsciousSettings::default(),
        learning: residuum_config::LearningConfig::default(),
        gateway: residuum_config::GatewayConfig::default(),
        timezone: chrono_tz::UTC,
        cloud: None,
        discord: None,
        telegram: None,
        teams: None,
        a2a: residuum_config::A2aConfig::default(),
        webhooks: std::collections::HashMap::new(),
        skills: residuum_config::SkillsConfig { dirs: vec![] },
        tools: residuum_config::ToolsConfig { dirs: vec![] },
        retry: residuum_config::RetryConfig::default(),
        background: residuum_config::BackgroundConfig::default(),
        agent: residuum_config::AgentAbilitiesConfig::default(),
        auto_mode: residuum_config::AutoModeConfig::default(),
        idle: residuum_config::IdleConfig::default(),
        temperature: None,
        thinking: None,
        web_search: residuum_config::WebSearchConfig::default(),
        tracing: residuum_config::TracingConfig::default(),
        role_overrides: std::collections::HashMap::new(),
        model_catalog: residuum_config::ModelCatalog::default(),
        config_dir: std::path::PathBuf::from("/tmp/config"),
        load_notices: vec![],
    }
}
