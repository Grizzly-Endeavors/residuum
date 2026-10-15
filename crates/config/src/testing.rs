//! Fixtures other crates' tests build a `Config` with (feature `test-support`).

/// The agent directory the fixtures resolve against (it need not exist).
#[must_use]
pub fn test_agent_dir() -> std::path::PathBuf {
    std::env::temp_dir().join("residuum-test-agent")
}

/// A hub config with every hub-owned value at its default and UTC as the
/// timezone.
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
        config_dir: std::env::temp_dir().join("residuum-test-hub"),
        load_notices: Vec::new(),
    }
}
