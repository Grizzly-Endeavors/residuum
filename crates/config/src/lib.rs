//! Configuration loading and validation.
//!
//! Uses a two-type pattern: raw TOML deserialization structs (in `deserialize`)
//! are validated into `Config` (runtime-safe values). Providers are defined in
//! `[providers]`, models are assigned to roles in `[models]`, and everything
//! resolves at load time into fully-built `ProviderSpec` values.
//!
//! The crate also holds [`agent_keys`], whose store encrypts with the secret
//! store's key handling.
//!
//! The `test-support` feature adds `WorkspaceLayout::with_tempdir_guard` and
//! `testing`, the fixtures other crates' tests build a `Config` with.

pub mod agent_keys;
mod agent_name;
mod bootstrap;
pub mod channels;
mod constants;
mod deserialize;
mod hub_load;
mod hub_types;
mod layout;
mod load;
mod model_catalog;
pub mod patch;
pub mod paths;
mod provider;
mod resolve;
mod retry;
pub mod secrets;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;
mod thinking;
mod tolerant;
mod types;

// ── Public re-exports ─────────────────────────────────────────────────────────

pub use agent_name::{
    allocate_slug, canonicalize_display_name, display_name_in_toml, display_name_key,
    set_display_name_toml, slug_base,
};
pub use channels::{ExternalChannelConfig, ExternalChannelKind};
pub use constants::{
    DEFAULT_A2A_PORT, DEFAULT_CLOUD_RELAY_URL, DEFAULT_GATEWAY_PORT,
    DEFAULT_OBSERVER_COOLDOWN_SECS, DEFAULT_OBSERVER_FORCE_THRESHOLD, DEFAULT_OBSERVER_THRESHOLD,
    DEFAULT_REFLECTOR_THRESHOLD, DEFAULT_SUBCONSCIOUS_EVERY_N_ITERATIONS,
    DEFAULT_SUBCONSCIOUS_MAX_TRANSCRIPT_TOKENS, DEFAULT_TEAMS_PORT, SLACK_ENDPOINT, TEAMS_ENDPOINT,
    WEB_UI_ENDPOINT,
};
pub use deserialize::{AgentConfigFile, HubConfigFile, ProvidersFile};
pub use hub_types::{
    HubA2aConfig, HubBackgroundConfig, HubConfig, HubPushConfig, SystemOneConfig, SystemOneProvider,
};
pub use layout::WorkspaceLayout;
pub use model_catalog::{ModelCatalog, SharedModelCatalog};
pub use paths::{HubPaths, default_hub_dir, discover_agents, residuum_root, validate_agent_name};
pub use provider::{ImageInput, ModelSpec, ProviderKind, ProviderSpec};
pub use resolve::{from_file_and_env, removed_agent_env_override_notices, resolve_hub_config};
pub use retry::RetryConfig;
pub use secrets::SecretStore;
pub use thinking::{ThinkingConfig, ThinkingLevel};
pub use types::{
    A2aConfig, A2aVisibility, AgentAbilitiesConfig, AutoModeConfig, BackgroundConfig,
    BackgroundModelTier, BackgroundModelsConfig, CloudConfig, Config, DiscordConfig, GatewayConfig,
    IdleConfig, LearningConfig, LogLevel, MemoryConfig, ModelSelector, OtelEndpoint,
    ProviderNativeSearchConfig, RemoteAccessSettings, RepeatCallGuardConfig, RoleOverrides,
    SearchConfig, SharedToolsPath, SkillDir, SkillSource, SkillsConfig, SlackConfig,
    StandaloneBackendConfig, SubconsciousSettings, TeamsConfig, TelegramConfig, ToolsConfig,
    TracingConfig, WebSearchConfig, WebhookEntry, WebhookFormat, WebhookRouting, apply_tools_path,
};
