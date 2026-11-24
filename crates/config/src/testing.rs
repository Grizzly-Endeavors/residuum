#[must_use]
pub fn test_agent_dir() -> std::path::PathBuf {
    std::env::temp_dir().join("residuum-test-agent")
}

#[must_use]
pub fn test_hub_config() -> crate::HubConfig {
    crate::HubConfig {
        timezone: chrono_tz::UTC,
        gateway: crate::GatewayConfig::default(),
        cloud: None,
        a2a: crate::HubA2aConfig::default(),
        tracing: crate::TracingConfig::default(),
        background: crate::HubBackgroundConfig::default(),
        push: crate::HubPushConfig::default(),
        system_one: None,
        pricing: std::collections::BTreeMap::new(),
        config_dir: std::env::temp_dir().join("residuum-test-hub"),
        load_notices: Vec::new(),
    }
}
