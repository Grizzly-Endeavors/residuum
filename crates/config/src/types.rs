use std::collections::HashMap;
use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::RetryConfig;

use super::constants::{
    DEFAULT_AGENT_MODIFY_CHANNELS, DEFAULT_AGENT_MODIFY_MCP, DEFAULT_EPISODE_SKIP_TOKEN_FLOOR,
    DEFAULT_FEEDBACK_ENDPOINT, DEFAULT_GATEWAY_BIND, DEFAULT_GATEWAY_PORT, DEFAULT_HOP_HARD_LIMIT,
    DEFAULT_HOP_SOFT_LIMIT, DEFAULT_IDLE_TIMEOUT_ARTIFACT_MINUTES,
    DEFAULT_IDLE_TIMEOUT_EXTERNAL_MINUTES, DEFAULT_IDLE_TIMEOUT_MINUTES,
    DEFAULT_IDLE_TIMEOUT_SCHEDULED_MINUTES, DEFAULT_IDLE_TIMEOUT_SPAWNED_MINUTES,
    DEFAULT_LEARNING_COOLDOWN_MINUTES, DEFAULT_LEARNING_NUDGE_AFTER_TURNS,
    DEFAULT_MAX_CONCURRENT_BACKGROUND, DEFAULT_OBSERVER_COOLDOWN_SECS,
    DEFAULT_OBSERVER_FORCE_THRESHOLD, DEFAULT_OBSERVER_THRESHOLD, DEFAULT_REFLECTOR_THRESHOLD,
    DEFAULT_REPEAT_CALL_STEER_AFTER, DEFAULT_REPEAT_CALL_STOP_AFTER,
    DEFAULT_SEARCH_CANDIDATE_MULTIPLIER, DEFAULT_SEARCH_MIN_SCORE, DEFAULT_SEARCH_TEMPORAL_DECAY,
    DEFAULT_SEARCH_TEMPORAL_DECAY_HALF_LIFE_DAYS, DEFAULT_SEARCH_TEXT_WEIGHT,
    DEFAULT_SEARCH_VECTOR_WEIGHT, DEFAULT_SUBAGENT_DEPTH_CAP,
    DEFAULT_SUBCONSCIOUS_EVERY_N_ITERATIONS, DEFAULT_SUBCONSCIOUS_MAX_TRANSCRIPT_TOKENS,
    WEB_UI_ENDPOINT,
};
use super::provider::ProviderSpec;

#[derive(Debug, Clone, PartialEq)]
pub struct GatewayConfig {
    pub bind: String,
    pub port: u16,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            bind: DEFAULT_GATEWAY_BIND.to_string(),
            port: DEFAULT_GATEWAY_PORT,
        }
    }
}

impl GatewayConfig {
    #[must_use]
    pub fn addr(&self) -> String {
        format!("{}:{}", self.bind, self.port)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MemoryConfig {
    pub observer_threshold_tokens: usize,
    pub reflector_threshold_tokens: usize,
    pub observer_cooldown_secs: u64,
    pub observer_force_threshold_tokens: usize,
    pub search: SearchConfig,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            observer_threshold_tokens: DEFAULT_OBSERVER_THRESHOLD,
            reflector_threshold_tokens: DEFAULT_REFLECTOR_THRESHOLD,
            observer_cooldown_secs: DEFAULT_OBSERVER_COOLDOWN_SECS,
            observer_force_threshold_tokens: DEFAULT_OBSERVER_FORCE_THRESHOLD,
            search: SearchConfig::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchConfig {
    pub vector_weight: f64,
    pub text_weight: f64,
    pub min_score: f64,
    pub candidate_multiplier: usize,
    pub temporal_decay: bool,
    pub temporal_decay_half_life_days: f64,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            vector_weight: DEFAULT_SEARCH_VECTOR_WEIGHT,
            text_weight: DEFAULT_SEARCH_TEXT_WEIGHT,
            min_score: DEFAULT_SEARCH_MIN_SCORE,
            candidate_multiplier: DEFAULT_SEARCH_CANDIDATE_MULTIPLIER,
            temporal_decay: DEFAULT_SEARCH_TEMPORAL_DECAY,
            temporal_decay_half_life_days: DEFAULT_SEARCH_TEMPORAL_DECAY_HALF_LIFE_DAYS,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CloudConfig {
    pub relay_url: String,
    pub token: String,
    pub remote: RemoteAccessSettings,
}

pub const ACME_PRODUCTION_DIRECTORY: &str = "https://acme-v02.api.letsencrypt.org/directory";

pub const ACME_STAGING_DIRECTORY: &str = "https://acme-staging-v02.api.letsencrypt.org/directory";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteAccessSettings {
    pub base_domain: String,
    pub acme_directory: String,
    pub acme_root_ca: Option<std::path::PathBuf>,
    pub pin_service_url: String,
    pub caa_resolver: std::net::SocketAddr,
}

impl Default for RemoteAccessSettings {
    fn default() -> Self {
        Self {
            base_domain: "agent-residuum.com".to_string(),
            acme_directory: ACME_PRODUCTION_DIRECTORY.to_string(),
            acme_root_ca: None,
            pin_service_url: "https://pins.agent-residuum.com".to_string(),
            caa_resolver: std::net::SocketAddr::from(([1, 1, 1, 1], 53)),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct DiscordConfig {
    pub token: String,
    pub respond_to_others: bool,
    pub context_messages: usize,
    pub deliver_intermediate: bool,
}

impl std::fmt::Debug for DiscordConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiscordConfig")
            .field("token", &"[redacted]")
            .field("respond_to_others", &self.respond_to_others)
            .field("context_messages", &self.context_messages)
            .field("deliver_intermediate", &self.deliver_intermediate)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct TelegramConfig {
    pub token: String,
    pub respond_to_others: bool,
    pub context_messages: usize,
    pub deliver_intermediate: bool,
}

impl std::fmt::Debug for TelegramConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TelegramConfig")
            .field("token", &"[redacted]")
            .field("respond_to_others", &self.respond_to_others)
            .field("context_messages", &self.context_messages)
            .field("deliver_intermediate", &self.deliver_intermediate)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct TeamsConfig {
    pub app_id: String,
    pub app_password: String,
    /// Directory (tenant) ID; inbound activities from other tenants are rejected.
    pub tenant_id: String,
    pub respond_to_others: bool,
    pub context_messages: usize,
    pub deliver_intermediate: bool,
    pub port: u16,
}

impl std::fmt::Debug for TeamsConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TeamsConfig")
            .field("app_id", &self.app_id)
            .field("app_password", &"[redacted]")
            .field("tenant_id", &self.tenant_id)
            .field("respond_to_others", &self.respond_to_others)
            .field("context_messages", &self.context_messages)
            .field("deliver_intermediate", &self.deliver_intermediate)
            .field("port", &self.port)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum A2aVisibility {
    #[default]
    Public,
    /// Every route, including the Agent Card, answers a plain 404 to a
    /// caller without a valid key — indistinguishable
    /// from an agent that doesn't exist.
    Private,
}

impl A2aVisibility {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Private => "private",
        }
    }
}

impl std::fmt::Display for A2aVisibility {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct A2aConfig {
    pub enabled: bool,
    pub port: u16,
    pub public_url: Option<String>,
    pub visibility: A2aVisibility,
}

impl Default for A2aConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            port: super::constants::DEFAULT_A2A_PORT,
            public_url: None,
            visibility: A2aVisibility::default(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum WebhookRouting {
    #[default]
    Inbox,
    Agent(String),
}

impl fmt::Display for WebhookRouting {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Inbox => f.write_str("inbox"),
            Self::Agent(name) => write!(f, "agent:{name}"),
        }
    }
}

impl std::str::FromStr for WebhookRouting {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "inbox" => Ok(Self::Inbox),
            other => match other.strip_prefix("agent:") {
                Some(name) if !name.is_empty() => Ok(Self::Agent(name.to_string())),
                _ => Err(format!(
                    "invalid webhook routing '{s}': must be \"inbox\" or \"agent:<name>\""
                )),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum WebhookFormat {
    #[default]
    Parsed,
    Raw,
}

impl fmt::Display for WebhookFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parsed => f.write_str("parsed"),
            Self::Raw => f.write_str("raw"),
        }
    }
}

impl std::str::FromStr for WebhookFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "parsed" => Ok(Self::Parsed),
            "raw" => Ok(Self::Raw),
            other => Err(format!(
                "invalid webhook format '{other}': must be \"parsed\" or \"raw\""
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WebhookEntry {
    pub secret: Option<String>,
    pub routing: WebhookRouting,
    pub format: WebhookFormat,
    pub content_fields: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillSource {
    Agent,
    Team,
    Configured,
}

impl fmt::Display for SkillSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Agent => write!(f, "agent"),
            Self::Team => write!(f, "team"),
            Self::Configured => write!(f, "configured"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillDir {
    pub path: PathBuf,
    pub source: SkillSource,
}

impl SkillDir {
    #[must_use]
    pub fn new(path: impl Into<PathBuf>, source: SkillSource) -> Self {
        Self {
            path: path.into(),
            source,
        }
    }

    #[must_use]
    pub fn agent(path: impl Into<PathBuf>) -> Self {
        Self::new(path, SkillSource::Agent)
    }

    #[must_use]
    pub fn team(path: impl Into<PathBuf>) -> Self {
        Self::new(path, SkillSource::Team)
    }

    #[must_use]
    pub fn configured(path: impl Into<PathBuf>) -> Self {
        Self::new(path, SkillSource::Configured)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SkillsConfig {
    pub dirs: Vec<SkillDir>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolsConfig {
    pub dirs: Vec<PathBuf>,
}

impl ToolsConfig {
    #[must_use]
    pub fn effective_path(&self) -> Option<OsString> {
        if self.dirs.is_empty() {
            return None;
        }
        let mut parts: Vec<PathBuf> = self.dirs.clone();
        if let Some(inherited) = std::env::var_os("PATH") {
            parts.extend(std::env::split_paths(&inherited));
        }
        std::env::join_paths(parts).ok()
    }
}

pub type SharedToolsPath = Arc<RwLock<Option<OsString>>>;

pub async fn apply_tools_path(cmd: &mut tokio::process::Command, tools_path: &SharedToolsPath) {
    if let Some(path) = tools_path.read().await.as_ref() {
        cmd.env("PATH", path);
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentAbilitiesConfig {
    pub modify_mcp: bool,
    pub modify_channels: bool,
    pub max_tool_iterations: Option<usize>,
    pub repeat_call_guard: RepeatCallGuardConfig,
}

impl Default for AgentAbilitiesConfig {
    fn default() -> Self {
        Self {
            modify_mcp: DEFAULT_AGENT_MODIFY_MCP,
            modify_channels: DEFAULT_AGENT_MODIFY_CHANNELS,
            max_tool_iterations: None,
            repeat_call_guard: RepeatCallGuardConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepeatCallGuardConfig {
    pub enabled: bool,
    pub steer_after: u32,
    pub stop_after: u32,
}

impl Default for RepeatCallGuardConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            steer_after: DEFAULT_REPEAT_CALL_STEER_AFTER,
            stop_after: DEFAULT_REPEAT_CALL_STOP_AFTER,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AutoModeConfig {
    pub enabled: bool,
    pub deny: Vec<String>,
    pub allow: Vec<String>,
    pub threshold: f64,
}

pub const DEFAULT_AUTO_MODE_THRESHOLD: f64 = 0.5;

impl Default for AutoModeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            deny: Vec::new(),
            allow: Vec::new(),
            threshold: DEFAULT_AUTO_MODE_THRESHOLD,
        }
    }
}

impl AutoModeConfig {
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.enabled && !self.deny.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct IdleConfig {
    pub timeout: Duration,
    pub idle_channel: String,
}

impl Default for IdleConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(DEFAULT_IDLE_TIMEOUT_MINUTES * 60),
            idle_channel: WEB_UI_ENDPOINT.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SubconsciousSettings {
    pub enabled: bool,
    pub mid_turn: bool,
    pub every_n_iterations: usize,
    pub max_transcript_tokens: usize,
    pub learning: bool,
    pub learning_cooldown_minutes: u64,
}

impl Default for SubconsciousSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            mid_turn: true,
            every_n_iterations: DEFAULT_SUBCONSCIOUS_EVERY_N_ITERATIONS,
            max_transcript_tokens: DEFAULT_SUBCONSCIOUS_MAX_TRANSCRIPT_TOKENS,
            learning: false,
            learning_cooldown_minutes: DEFAULT_LEARNING_COOLDOWN_MINUTES,
        }
    }
}

impl SubconsciousSettings {
    #[must_use]
    pub fn learning_cooldown(&self) -> Duration {
        Duration::from_secs(self.learning_cooldown_minutes.saturating_mul(60))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LearningConfig {
    pub nudge_after_turns: u32,
}

impl Default for LearningConfig {
    fn default() -> Self {
        Self {
            nudge_after_turns: DEFAULT_LEARNING_NUDGE_AFTER_TURNS,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BackgroundConfig {
    pub max_concurrent: usize,
    pub models: BackgroundModelsConfig,
    pub idle_timeout_scheduled: Duration,
    pub idle_timeout_spawned: Duration,
    pub idle_timeout_external: Duration,
    pub idle_timeout_artifact: Duration,
    pub episode_skip_token_floor: usize,
    pub subagent_depth_cap: u32,
    pub hop_soft_limit: u32,
    pub hop_hard_limit: u32,
}

impl Default for BackgroundConfig {
    fn default() -> Self {
        Self {
            max_concurrent: DEFAULT_MAX_CONCURRENT_BACKGROUND,
            models: BackgroundModelsConfig::default(),
            idle_timeout_scheduled: Duration::from_secs(
                DEFAULT_IDLE_TIMEOUT_SCHEDULED_MINUTES * 60,
            ),
            idle_timeout_spawned: Duration::from_secs(DEFAULT_IDLE_TIMEOUT_SPAWNED_MINUTES * 60),
            idle_timeout_external: Duration::from_secs(DEFAULT_IDLE_TIMEOUT_EXTERNAL_MINUTES * 60),
            idle_timeout_artifact: Duration::from_secs(DEFAULT_IDLE_TIMEOUT_ARTIFACT_MINUTES * 60),
            episode_skip_token_floor: DEFAULT_EPISODE_SKIP_TOKEN_FLOOR,
            subagent_depth_cap: DEFAULT_SUBAGENT_DEPTH_CAP,
            hop_soft_limit: DEFAULT_HOP_SOFT_LIMIT,
            hop_hard_limit: DEFAULT_HOP_HARD_LIMIT,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct BackgroundModelsConfig {
    pub small: Option<Vec<ProviderSpec>>,
    pub medium: Option<Vec<ProviderSpec>>,
    pub large: Option<Vec<ProviderSpec>>,
}

impl BackgroundModelsConfig {
    #[must_use]
    pub fn resolve_tier(
        &self,
        tier: &BackgroundModelTier,
        main: &[ProviderSpec],
    ) -> Vec<ProviderSpec> {
        match tier {
            BackgroundModelTier::Small => self
                .small
                .as_ref()
                .or(self.medium.as_ref())
                .or(self.large.as_ref())
                .cloned()
                .unwrap_or_else(|| main.to_vec()),
            BackgroundModelTier::Medium => self
                .medium
                .as_ref()
                .or(self.large.as_ref())
                .cloned()
                .unwrap_or_else(|| main.to_vec()),
            BackgroundModelTier::Large => self.large.clone().unwrap_or_else(|| main.to_vec()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundModelTier {
    Small,
    #[default]
    Medium,
    Large,
}

impl std::fmt::Display for BackgroundModelTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Small => f.write_str("small"),
            Self::Medium => f.write_str("medium"),
            Self::Large => f.write_str("large"),
        }
    }
}

impl std::str::FromStr for BackgroundModelTier {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "small" => Ok(Self::Small),
            "medium" => Ok(Self::Medium),
            "large" => Ok(Self::Large),
            other => Err(format!(
                "invalid model tier '{other}': must be small, medium, or large"
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelSelector {
    Tier(BackgroundModelTier),
    Explicit(String),
}

impl ModelSelector {
    pub const SMALL: Self = Self::Tier(BackgroundModelTier::Small);
    pub const MEDIUM: Self = Self::Tier(BackgroundModelTier::Medium);
    pub const LARGE: Self = Self::Tier(BackgroundModelTier::Large);
}

impl Default for ModelSelector {
    fn default() -> Self {
        Self::MEDIUM
    }
}

impl From<BackgroundModelTier> for ModelSelector {
    fn from(tier: BackgroundModelTier) -> Self {
        Self::Tier(tier)
    }
}

impl fmt::Display for ModelSelector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tier(tier) => tier.fmt(f),
            Self::Explicit(id) => f.write_str(id),
        }
    }
}

impl std::str::FromStr for ModelSelector {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Ok(tier) = s.parse::<BackgroundModelTier>() {
            return Ok(Self::Tier(tier));
        }
        let looks_like_model_id = s.split_once('/').is_some_and(|(provider, model)| {
            !provider.is_empty() && !model.is_empty() && !s.contains(char::is_whitespace)
        });
        if looks_like_model_id {
            Ok(Self::Explicit(s.to_string()))
        } else {
            Err(format!(
                "invalid model '{s}': use a tier (small, medium, or large) or a model id in \
                 provider/model form, such as anthropic/claude-haiku-4-5"
            ))
        }
    }
}

impl Serialize for ModelSelector {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ModelSelector {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        raw.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RoleOverrides {
    pub temperature: Option<f32>,
    pub thinking: Option<crate::ThinkingConfig>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    #[default]
    Debug,
    Trace,
}

impl LogLevel {
    #[must_use]
    pub fn filter_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Debug => "residuum=debug,warn",
            Self::Trace => "residuum=trace,warn",
        }
    }
}

impl fmt::Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Info => f.write_str("info"),
            Self::Debug => f.write_str("debug"),
            Self::Trace => f.write_str("trace"),
        }
    }
}

impl std::str::FromStr for LogLevel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "info" => Ok(Self::Info),
            "debug" => Ok(Self::Debug),
            "trace" => Ok(Self::Trace),
            other => Err(format!(
                "invalid log level '{other}': must be info, debug, or trace"
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OtelEndpoint {
    pub url: String,
    pub name: Option<String>,
    pub headers: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TracingConfig {
    pub log_level: LogLevel,
    pub auto_error_reporting: bool,
    pub sanitize_content: bool,
    pub otel_endpoints: Vec<OtelEndpoint>,
    pub feedback_endpoint: String,
}

impl Default for TracingConfig {
    fn default() -> Self {
        Self {
            log_level: LogLevel::default(),
            auto_error_reporting: false,
            sanitize_content: true,
            otel_endpoints: Vec::new(),
            feedback_endpoint: DEFAULT_FEEDBACK_ENDPOINT.to_string(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct WebSearchConfig {
    pub provider_native: Option<ProviderNativeSearchConfig>,
    pub standalone_backend: Option<StandaloneBackendConfig>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProviderNativeSearchConfig {
    /// Maximum web search invocations per request (Anthropic).
    pub max_uses: Option<u32>,
    pub allowed_domains: Option<Vec<String>>,
    pub blocked_domains: Option<Vec<String>>,
    /// Search context size (`OpenAI`: `"low"`, `"medium"`, `"high"`).
    pub search_context_size: Option<String>,
    pub exclude_domains: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StandaloneBackendConfig {
    pub name: String,
    pub api_key: String,
    /// Base URL (only used by Ollama Cloud).
    pub base_url: Option<String>,
}

#[derive(Clone, PartialEq)]
pub struct Config {
    pub agent_name: String,
    pub autostart: bool,
    pub main: Vec<ProviderSpec>,
    pub observer: Vec<ProviderSpec>,
    pub reflector: Vec<ProviderSpec>,
    pub pulse: Vec<ProviderSpec>,
    pub subconscious: Vec<ProviderSpec>,
    pub embedding: Option<ProviderSpec>,
    pub workspace_dir: PathBuf,
    pub timeout_secs: u64,
    pub max_tokens: u32,
    pub memory: MemoryConfig,
    pub pulse_enabled: bool,
    pub subconscious_settings: SubconsciousSettings,
    pub learning: LearningConfig,
    pub gateway: GatewayConfig,
    pub timezone: chrono_tz::Tz,
    pub cloud: Option<CloudConfig>,
    pub discord: Option<DiscordConfig>,
    pub telegram: Option<TelegramConfig>,
    pub teams: Option<TeamsConfig>,
    pub a2a: A2aConfig,
    pub webhooks: HashMap<String, WebhookEntry>,
    pub skills: SkillsConfig,
    pub tools: ToolsConfig,
    pub retry: RetryConfig,
    pub background: BackgroundConfig,
    pub agent: AgentAbilitiesConfig,
    pub auto_mode: AutoModeConfig,
    pub idle: IdleConfig,
    pub temperature: Option<f32>,
    pub thinking: Option<crate::ThinkingConfig>,
    pub web_search: WebSearchConfig,
    pub tracing: TracingConfig,
    pub role_overrides: HashMap<String, RoleOverrides>,
    pub model_catalog: super::model_catalog::ModelCatalog,
    pub config_dir: PathBuf,
    pub load_notices: Vec<String>,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("agent_name", &self.agent_name)
            .field("autostart", &self.autostart)
            .field("main", &self.main)
            .field("observer", &self.observer)
            .field("reflector", &self.reflector)
            .field("pulse", &self.pulse)
            .field("subconscious", &self.subconscious)
            .field("embedding", &self.embedding)
            .field("workspace_dir", &self.workspace_dir)
            .field("timeout_secs", &self.timeout_secs)
            .field("max_tokens", &self.max_tokens)
            .field("memory", &self.memory)
            .field("pulse_enabled", &self.pulse_enabled)
            .field("subconscious_settings", &self.subconscious_settings)
            .field("learning", &self.learning)
            .field("gateway", &self.gateway)
            .field("timezone", &self.timezone)
            .field("cloud", &self.cloud.as_ref().map(|_| "[configured]"))
            .field("discord", &self.discord.as_ref().map(|_| "[configured]"))
            .field("telegram", &self.telegram.as_ref().map(|_| "[configured]"))
            .field("teams", &self.teams)
            .field("a2a", &self.a2a)
            .field(
                "webhooks",
                &format_args!("{} configured", self.webhooks.len()),
            )
            .field("skills", &self.skills)
            .field("tools", &self.tools)
            .field("retry", &self.retry)
            .field("background", &self.background)
            .field("agent", &self.agent)
            .field("auto_mode", &self.auto_mode)
            .field("idle", &self.idle)
            .field("temperature", &self.temperature)
            .field("thinking", &self.thinking)
            .field("web_search", &self.web_search)
            .field("tracing", &self.tracing)
            .field("role_overrides", &self.role_overrides)
            .field("model_catalog", &self.model_catalog)
            .field("config_dir", &self.config_dir)
            .field("load_notices", &self.load_notices)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_selector_parses_tiers() {
        assert_eq!("small".parse(), Ok(ModelSelector::SMALL));
        assert_eq!("medium".parse(), Ok(ModelSelector::MEDIUM));
        assert_eq!("large".parse(), Ok(ModelSelector::LARGE));
    }

    #[test]
    fn model_selector_parses_explicit_ids() {
        assert_eq!(
            "anthropic/claude-haiku-4-5".parse(),
            Ok(ModelSelector::Explicit(
                "anthropic/claude-haiku-4-5".to_string()
            ))
        );
        assert_eq!(
            "router/meta-llama/llama-3".parse(),
            Ok(ModelSelector::Explicit(
                "router/meta-llama/llama-3".to_string()
            ))
        );
    }

    #[test]
    fn model_selector_rejects_anything_else_naming_the_value() {
        for bad in ["smal", "SMALL", "", "/model", "provider/", "a b/c"] {
            let err = bad.parse::<ModelSelector>().unwrap_err();
            assert!(err.contains(&format!("'{bad}'")), "{err}");
        }
    }

    #[test]
    fn model_selector_round_trips_as_a_plain_string() {
        for selector in [
            ModelSelector::LARGE,
            ModelSelector::Explicit("local/qwen3:8b".to_string()),
        ] {
            let json = serde_json::to_string(&selector).unwrap();
            assert_eq!(json, format!("\"{selector}\""));
            let back: ModelSelector = serde_json::from_str(&json).unwrap();
            assert_eq!(back, selector);
        }
        assert!(serde_json::from_str::<ModelSelector>("\"huge\"").is_err());
    }

    #[test]
    fn effective_path_empty_dirs_is_none() {
        let cfg = ToolsConfig { dirs: vec![] };
        assert!(
            cfg.effective_path().is_none(),
            "no tool dirs means no PATH override"
        );
    }

    #[test]
    fn effective_path_prepends_dirs_in_order() {
        let cfg = ToolsConfig {
            dirs: vec![
                PathBuf::from("/opt/tools"),
                PathBuf::from("/home/x/.residuum/bin"),
            ],
        };
        let path = cfg.effective_path().unwrap();
        let entries: Vec<PathBuf> = std::env::split_paths(&path).collect();
        assert_eq!(
            entries.first(),
            Some(&PathBuf::from("/opt/tools")),
            "configured dir comes first"
        );
        assert_eq!(
            entries.get(1),
            Some(&PathBuf::from("/home/x/.residuum/bin")),
            "default bin comes after configured dirs"
        );
        if let Some(inherited) = std::env::var_os("PATH") {
            let inherited_first = std::env::split_paths(&inherited).next();
            if let Some(first) = inherited_first {
                assert_eq!(
                    entries.get(2),
                    Some(&first),
                    "inherited PATH follows the tool dirs"
                );
            }
        }
    }

    #[test]
    fn from_str_valid_tiers() {
        assert_eq!(
            "small".parse::<BackgroundModelTier>().unwrap(),
            BackgroundModelTier::Small
        );
        assert_eq!(
            "medium".parse::<BackgroundModelTier>().unwrap(),
            BackgroundModelTier::Medium
        );
        assert_eq!(
            "large".parse::<BackgroundModelTier>().unwrap(),
            BackgroundModelTier::Large
        );
    }

    #[test]
    fn from_str_invalid() {
        assert!("invalid".parse::<BackgroundModelTier>().is_err());
        assert!("SMALL".parse::<BackgroundModelTier>().is_err());
        assert!("".parse::<BackgroundModelTier>().is_err());
    }

    #[test]
    fn display_round_trips() {
        for tier in [
            BackgroundModelTier::Small,
            BackgroundModelTier::Medium,
            BackgroundModelTier::Large,
        ] {
            let s = tier.to_string();
            assert_eq!(s.parse::<BackgroundModelTier>().unwrap(), tier);
        }
    }

    #[test]
    fn webhook_routing_from_str() {
        assert_eq!(
            "inbox".parse::<super::WebhookRouting>().unwrap(),
            super::WebhookRouting::Inbox
        );
        assert_eq!(
            "agent:code_reviewer"
                .parse::<super::WebhookRouting>()
                .unwrap(),
            super::WebhookRouting::Agent("code_reviewer".to_string())
        );
        assert!("agent:".parse::<super::WebhookRouting>().is_err());
        assert!("invalid".parse::<super::WebhookRouting>().is_err());
        assert!("".parse::<super::WebhookRouting>().is_err());
    }

    #[test]
    fn webhook_routing_display_round_trips() {
        for routing in [
            super::WebhookRouting::Inbox,
            super::WebhookRouting::Agent("test".to_string()),
        ] {
            let s = routing.to_string();
            assert_eq!(s.parse::<super::WebhookRouting>().unwrap(), routing);
        }
    }

    #[test]
    fn webhook_format_from_str() {
        assert_eq!(
            "parsed".parse::<super::WebhookFormat>().unwrap(),
            super::WebhookFormat::Parsed
        );
        assert_eq!(
            "raw".parse::<super::WebhookFormat>().unwrap(),
            super::WebhookFormat::Raw
        );
        assert!("invalid".parse::<super::WebhookFormat>().is_err());
    }

    #[test]
    fn webhook_format_display_round_trips() {
        for fmt in [super::WebhookFormat::Parsed, super::WebhookFormat::Raw] {
            let s = fmt.to_string();
            assert_eq!(s.parse::<super::WebhookFormat>().unwrap(), fmt);
        }
    }

    fn dummy_spec(tier: &str) -> ProviderSpec {
        use super::super::provider::{ModelSpec, ProviderKind};

        ProviderSpec {
            name: format!("dummy-{tier}"),
            model: ModelSpec {
                kind: ProviderKind::OpenAi,
                model: format!("{tier}-model"),
            },
            provider_url: "http://dummy".to_string(),
            api_key: None,
            keep_alive: None,
            session_affinity: None,
            image_input: crate::ImageInput::Undeclared,
        }
    }

    #[test]
    fn resolve_tier_tests() {
        let p_small = dummy_spec("small");
        let p_medium = dummy_spec("medium");
        let p_large = dummy_spec("large");
        let p_main = dummy_spec("main");

        let main_slice = std::slice::from_ref(&p_main);

        let config_full = BackgroundModelsConfig {
            small: Some(vec![p_small.clone()]),
            medium: Some(vec![p_medium.clone()]),
            large: Some(vec![p_large.clone()]),
        };
        assert_eq!(
            config_full.resolve_tier(&BackgroundModelTier::Small, main_slice),
            vec![p_small.clone()]
        );
        assert_eq!(
            config_full.resolve_tier(&BackgroundModelTier::Medium, main_slice),
            vec![p_medium.clone()]
        );
        assert_eq!(
            config_full.resolve_tier(&BackgroundModelTier::Large, main_slice),
            vec![p_large.clone()]
        );

        let config_no_small = BackgroundModelsConfig {
            small: None,
            medium: Some(vec![p_medium.clone()]),
            large: Some(vec![p_large.clone()]),
        };
        assert_eq!(
            config_no_small.resolve_tier(&BackgroundModelTier::Small, main_slice),
            vec![p_medium.clone()]
        );

        let config_only_large = BackgroundModelsConfig {
            small: None,
            medium: None,
            large: Some(vec![p_large.clone()]),
        };
        assert_eq!(
            config_only_large.resolve_tier(&BackgroundModelTier::Small, main_slice),
            vec![p_large.clone()]
        );
        assert_eq!(
            config_only_large.resolve_tier(&BackgroundModelTier::Medium, main_slice),
            vec![p_large.clone()]
        );

        let config_empty = BackgroundModelsConfig::default();
        assert_eq!(
            config_empty.resolve_tier(&BackgroundModelTier::Small, main_slice),
            vec![p_main.clone()]
        );
        assert_eq!(
            config_empty.resolve_tier(&BackgroundModelTier::Medium, main_slice),
            vec![p_main.clone()]
        );
        assert_eq!(
            config_empty.resolve_tier(&BackgroundModelTier::Large, main_slice),
            vec![p_main.clone()]
        );
    }

    #[test]
    fn resolve_tier_only_small_and_only_medium() {
        use super::super::provider::{ModelSpec, ProviderKind};

        let p_small = ProviderSpec {
            name: "dummy-small".to_string(),
            model: ModelSpec {
                kind: ProviderKind::OpenAi,
                model: "small-model".to_string(),
            },
            provider_url: "http://dummy".to_string(),
            api_key: None,
            keep_alive: None,
            session_affinity: None,
            image_input: crate::ImageInput::Undeclared,
        };
        let p_medium = ProviderSpec {
            name: "dummy-medium".to_string(),
            model: ModelSpec {
                kind: ProviderKind::OpenAi,
                model: "medium-model".to_string(),
            },
            provider_url: "http://dummy".to_string(),
            api_key: None,
            keep_alive: None,
            session_affinity: None,
            image_input: crate::ImageInput::Undeclared,
        };
        let p_main = ProviderSpec {
            name: "dummy-main".to_string(),
            model: ModelSpec {
                kind: ProviderKind::OpenAi,
                model: "main-model".to_string(),
            },
            provider_url: "http://dummy".to_string(),
            api_key: None,
            keep_alive: None,
            session_affinity: None,
            image_input: crate::ImageInput::Undeclared,
        };

        let main_slice = std::slice::from_ref(&p_main);

        let config_only_small = BackgroundModelsConfig {
            small: Some(vec![p_small.clone()]),
            medium: None,
            large: None,
        };
        assert_eq!(
            config_only_small.resolve_tier(&BackgroundModelTier::Small, main_slice),
            vec![p_small.clone()]
        );
        assert_eq!(
            config_only_small.resolve_tier(&BackgroundModelTier::Medium, main_slice),
            vec![p_main.clone()]
        );
        assert_eq!(
            config_only_small.resolve_tier(&BackgroundModelTier::Large, main_slice),
            vec![p_main.clone()]
        );

        let config_only_medium = BackgroundModelsConfig {
            small: None,
            medium: Some(vec![p_medium.clone()]),
            large: None,
        };
        assert_eq!(
            config_only_medium.resolve_tier(&BackgroundModelTier::Small, main_slice),
            vec![p_medium.clone()]
        );
        assert_eq!(
            config_only_medium.resolve_tier(&BackgroundModelTier::Medium, main_slice),
            vec![p_medium.clone()]
        );
        assert_eq!(
            config_only_medium.resolve_tier(&BackgroundModelTier::Large, main_slice),
            vec![p_main.clone()]
        );
    }
}
