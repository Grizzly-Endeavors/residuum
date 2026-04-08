//! Configuration loading and validation.
//!
//! Uses a two-type pattern: raw TOML deserialization structs (in `deserialize`)
//! are validated into `Config` (runtime-safe values). Providers are defined in
//! `[providers]`, models are assigned to roles in `[models]`, and everything
//! resolves at load time into fully-built `ProviderSpec` values.

mod bootstrap;
mod constants;
pub(crate) mod deserialize;
mod load;
mod provider;
pub(crate) mod resolve;
pub(crate) mod secrets;
mod types;
pub mod wizard;

// ── Public re-exports ─────────────────────────────────────────────────────────

pub(crate) use constants::{
    DEFAULT_OBSERVER_COOLDOWN_SECS, DEFAULT_OBSERVER_FORCE_THRESHOLD, DEFAULT_OBSERVER_THRESHOLD,
    DEFAULT_REFLECTOR_THRESHOLD,
};
pub use provider::{ModelSpec, ProviderKind, ProviderSpec};
pub use secrets::SecretStore;
pub use types::{
    AgentAbilitiesConfig, BackgroundConfig, BackgroundModelTier, BackgroundModelsConfig,
    CloudConfig, Config, DiscordConfig, GatewayConfig, IdleConfig, LogLevel, MemoryConfig,
    OtelEndpoint, ProviderNativeSearchConfig, RoleOverrides, SearchConfig, SkillsConfig,
    StandaloneBackendConfig, TelegramConfig, TracingConfig, WebSearchConfig, WebhookEntry,
    WebhookFormat, WebhookRouting,
};
