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
mod load;
pub(crate) mod patch;
pub mod paths;
mod provider;
pub(crate) mod resolve;
pub(crate) mod secrets;
mod tolerant;
mod types;
pub mod wizard;

// ── Public re-exports ─────────────────────────────────────────────────────────

pub(crate) use agent_name::display_name_in_toml;
pub use agent_name::{
    allocate_slug, canonicalize_display_name, display_name_key, set_display_name_toml, slug_base,
};
pub(crate) use constants::{
    DEFAULT_A2A_PORT, DEFAULT_OBSERVER_COOLDOWN_SECS, DEFAULT_OBSERVER_FORCE_THRESHOLD,
    DEFAULT_OBSERVER_THRESHOLD, DEFAULT_REFLECTOR_THRESHOLD,
    DEFAULT_SUBCONSCIOUS_EVERY_N_ITERATIONS, DEFAULT_SUBCONSCIOUS_MAX_TRANSCRIPT_TOKENS,
    DEFAULT_TEAMS_PORT,
};
pub use hub_types::{
    HubA2aConfig, HubBackgroundConfig, HubConfig, HubPushConfig, SystemOneConfig, SystemOneProvider,
};
pub use paths::{HubPaths, default_hub_dir, discover_agents, residuum_root, validate_agent_name};
pub use provider::{ModelSpec, ProviderKind, ProviderSpec};
pub use secrets::SecretStore;
pub use types::{
    A2aConfig, A2aVisibility, AgentAbilitiesConfig, BackgroundConfig, BackgroundModelTier,
    BackgroundModelsConfig, CloudConfig, Config, DiscordConfig, GatewayConfig, IdleConfig,
    LearningConfig, LogLevel, MemoryConfig, OtelEndpoint, ProviderNativeSearchConfig,
    RepeatCallGuardConfig, RoleOverrides, SearchConfig, SkillsConfig, StandaloneBackendConfig,
    SubconsciousSettings, TeamsConfig, TelegramConfig, ToolsConfig, TracingConfig, WebSearchConfig,
    WebhookEntry, WebhookFormat, WebhookRouting,
};
