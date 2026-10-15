//! Configuration loading and validation.
//!
//! Uses a two-type pattern: raw TOML deserialization structs (in `deserialize`)
//! are validated into `Config` (runtime-safe values). Providers are defined in
//! `[providers]`, models are assigned to roles in `[models]`, and everything
//! resolves at load time into fully-built `ProviderSpec` values.

mod agent_name;
mod bootstrap;
mod constants;
pub(crate) mod deserialize;
mod hub_load;
mod hub_types;
mod layout;
mod load;
mod model_catalog;
pub(crate) mod patch;
pub mod paths;
mod provider;
pub(crate) mod resolve;
mod retry;
pub(crate) mod secrets;
mod thinking;
mod tolerant;
mod types;

// ── Public re-exports ─────────────────────────────────────────────────────────

pub(crate) use agent_name::display_name_in_toml;
pub use agent_name::{
    allocate_slug, canonicalize_display_name, display_name_key, set_display_name_toml, slug_base,
};
pub use constants::WEB_UI_ENDPOINT;
pub(crate) use constants::{
    DEFAULT_A2A_PORT, DEFAULT_CLOUD_RELAY_URL, DEFAULT_GATEWAY_PORT,
    DEFAULT_OBSERVER_COOLDOWN_SECS, DEFAULT_OBSERVER_FORCE_THRESHOLD, DEFAULT_OBSERVER_THRESHOLD,
    DEFAULT_REFLECTOR_THRESHOLD, DEFAULT_SUBCONSCIOUS_EVERY_N_ITERATIONS,
    DEFAULT_SUBCONSCIOUS_MAX_TRANSCRIPT_TOKENS, DEFAULT_TEAMS_PORT,
};
pub use hub_types::{
    HubA2aConfig, HubBackgroundConfig, HubConfig, HubPushConfig, SystemOneConfig, SystemOneProvider,
};
pub use layout::WorkspaceLayout;
pub use model_catalog::{ModelCatalog, SharedModelCatalog};
pub use paths::{HubPaths, default_hub_dir, discover_agents, residuum_root, validate_agent_name};
pub use provider::{ImageInput, ModelSpec, ProviderKind, ProviderSpec};
pub use retry::RetryConfig;
pub use secrets::SecretStore;
pub use thinking::{ThinkingConfig, ThinkingLevel};
pub(crate) use types::apply_tools_path;
pub use types::{
    A2aConfig, A2aVisibility, AgentAbilitiesConfig, AutoModeConfig, BackgroundConfig,
    BackgroundModelTier, BackgroundModelsConfig, CloudConfig, Config, DiscordConfig, GatewayConfig,
    IdleConfig, LearningConfig, LogLevel, MemoryConfig, ModelSelector, OtelEndpoint,
    ProviderNativeSearchConfig, RemoteAccessSettings, RepeatCallGuardConfig, RoleOverrides,
    SearchConfig, SharedToolsPath, SkillDir, SkillSource, SkillsConfig, StandaloneBackendConfig,
    SubconsciousSettings, TeamsConfig, TelegramConfig, ToolsConfig, TracingConfig, WebSearchConfig,
    WebhookEntry, WebhookFormat, WebhookRouting,
};
