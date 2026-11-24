use std::collections::{BTreeMap, HashMap};

use serde::Deserialize;

use super::hub_types::ModelPrice;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HubConfigFile {
    pub(super) timezone: Option<String>,
    pub(super) gateway: Option<GatewayConfigFile>,
    pub(super) cloud: Option<CloudConfigFile>,
    pub(super) tracing: Option<TracingConfigFile>,
    pub(super) a2a: Option<HubA2aConfigFile>,
    pub(super) background: Option<HubBackgroundConfigFile>,
    pub(super) push: Option<HubPushConfigFile>,
    pub(super) system_one: Option<SystemOneConfigFile>,
    pub(super) pricing: Option<BTreeMap<String, ModelPrice>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SystemOneConfigFile {
    pub(super) provider: Option<String>,
    pub(super) model: Option<String>,
    pub(super) url: Option<String>,
    pub(super) api_key: Option<String>,
    pub(super) keep_alive: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HubPushConfigFile {
    pub(super) contact: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HubA2aConfigFile {
    pub(super) enabled: Option<bool>,
    pub(super) port: Option<u16>,
    pub(super) public_url: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HubBackgroundConfigFile {
    pub(super) max_concurrent: Option<usize>,
    pub(super) hop_soft_limit: Option<u32>,
    pub(super) hop_hard_limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentConfigFile {
    pub(super) timeout_secs: Option<u64>,
    pub(super) max_tokens: Option<u32>,
    pub(super) memory: Option<MemoryConfigFile>,
    pub(super) pulse: Option<PulseConfigFile>,
    pub(super) subconscious: Option<SubconsciousConfigFile>,
    pub(super) learning: Option<LearningConfigFile>,
    pub(super) autostart: Option<bool>,
    pub(super) display_name: Option<String>,
    pub(super) discord: Option<DiscordConfigFile>,
    pub(super) telegram: Option<TelegramConfigFile>,
    pub(super) teams: Option<TeamsConfigFile>,
    pub(super) webhooks: Option<HashMap<String, WebhookEntryFile>>,
    pub(super) skills: Option<SkillsConfigFile>,
    pub(super) tools: Option<ToolsConfigFile>,
    pub(super) retry: Option<RetryConfigFile>,
    pub(super) background: Option<AgentBackgroundConfigFile>,
    pub(super) agent: Option<AgentAbilitiesConfigFile>,
    pub(super) idle: Option<IdleConfigFile>,
    pub(super) temperature: Option<f32>,
    pub(super) thinking: Option<String>,
    pub(super) web_search: Option<WebSearchConfigFile>,
    pub(super) a2a: Option<AgentA2aConfigFile>,
    pub(super) auto_mode: Option<AutoModeConfigFile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AutoModeConfigFile {
    pub(super) enabled: Option<bool>,
    pub(super) deny: Option<Vec<String>>,
    pub(super) allow: Option<Vec<String>>,
    pub(super) threshold: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AgentA2aConfigFile {
    pub(super) visibility: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AgentBackgroundConfigFile {
    pub(super) idle_timeout_scheduled_minutes: Option<u64>,
    pub(super) idle_timeout_spawned_minutes: Option<u64>,
    pub(super) idle_timeout_external_minutes: Option<u64>,
    pub(super) idle_timeout_artifact_minutes: Option<u64>,
    pub(super) episode_skip_token_floor: Option<usize>,
    pub(super) subagent_depth_cap: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AgentAbilitiesConfigFile {
    pub(super) modify_mcp: Option<bool>,
    pub(super) modify_channels: Option<bool>,
    pub(super) max_tool_iterations: Option<usize>,
    pub(super) repeat_call_guard_enabled: Option<bool>,
    pub(super) repeat_call_steer_after: Option<u32>,
    pub(super) repeat_call_stop_after: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProvidersFile {
    pub(super) providers: Option<HashMap<String, ProviderEntryFile>>,
    pub(super) models: Option<ModelsConfigFile>,
    pub(super) background: Option<BackgroundProviderSection>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BackgroundProviderSection {
    pub(super) models: Option<BackgroundModelsFile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProviderEntryFile {
    #[serde(rename = "type")]
    pub(super) kind: String,
    pub(super) api_key: Option<String>,
    pub(super) url: Option<String>,
    pub(super) keep_alive: Option<String>,
    pub(super) models: Option<HashMap<String, ProviderModelEntryFile>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProviderModelEntryFile {
    pub(super) input: Option<Vec<InputModalityFile>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum InputModalityFile {
    Text,
    Image,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub(super) enum ModelStringList {
    Single(String),
    List(Vec<String>),
}

impl ModelStringList {
    #[must_use]
    pub(super) fn into_vec(self) -> Vec<String> {
        match self {
            Self::Single(s) => vec![s],
            Self::List(v) => v,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub(super) enum ModelAssignment {
    Simple(String),
    List(Vec<String>),
    WithOverrides(ModelAssignmentTable),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ModelAssignmentTable {
    pub(super) model: ModelStringList,
    pub(super) temperature: Option<f32>,
    pub(super) thinking: Option<String>,
}

impl ModelAssignment {
    #[must_use]
    pub(super) fn into_model_strings(self) -> Vec<String> {
        match self {
            Self::Simple(s) => vec![s],
            Self::List(v) => v,
            Self::WithOverrides(table) => table.model.into_vec(),
        }
    }

    #[must_use]
    pub(super) fn overrides(&self) -> (Option<f32>, Option<&str>) {
        match self {
            Self::WithOverrides(table) => (table.temperature, table.thinking.as_deref()),
            Self::Simple(_) | Self::List(_) => (None, None),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ModelsConfigFile {
    pub(super) main: Option<ModelAssignment>,
    pub(super) default: Option<ModelAssignment>,
    pub(super) observer: Option<ModelAssignment>,
    pub(super) reflector: Option<ModelAssignment>,
    pub(super) pulse: Option<ModelAssignment>,
    pub(super) subconscious: Option<ModelAssignment>,
    pub(super) embedding: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MemoryConfigFile {
    pub(super) observer_threshold_tokens: Option<usize>,
    pub(super) reflector_threshold_tokens: Option<usize>,
    pub(super) observer_cooldown_secs: Option<u64>,
    pub(super) observer_force_threshold_tokens: Option<usize>,
    pub(super) search: Option<SearchConfigFile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SearchConfigFile {
    pub(super) vector_weight: Option<f64>,
    pub(super) text_weight: Option<f64>,
    pub(super) min_score: Option<f64>,
    pub(super) candidate_multiplier: Option<usize>,
    pub(super) temporal_decay: Option<bool>,
    pub(super) temporal_decay_half_life_days: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PulseConfigFile {
    pub(super) enabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SubconsciousConfigFile {
    pub(super) enabled: Option<bool>,
    pub(super) mid_turn: Option<bool>,
    pub(super) every_n_iterations: Option<usize>,
    pub(super) max_transcript_tokens: Option<usize>,
    pub(super) learning: Option<bool>,
    pub(super) learning_cooldown_minutes: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LearningConfigFile {
    pub(super) nudge_after_turns: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GatewayConfigFile {
    pub(super) bind: Option<String>,
    pub(super) port: Option<u16>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DiscordConfigFile {
    pub(super) token: Option<String>,
    pub(super) respond_to_others: Option<bool>,
    pub(super) context_messages: Option<usize>,
    pub(super) deliver_intermediate: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TelegramConfigFile {
    pub(super) token: Option<String>,
    pub(super) respond_to_others: Option<bool>,
    pub(super) context_messages: Option<usize>,
    pub(super) deliver_intermediate: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TeamsConfigFile {
    pub(super) app_id: Option<String>,
    pub(super) app_password: Option<String>,
    pub(super) tenant_id: Option<String>,
    pub(super) respond_to_others: Option<bool>,
    pub(super) context_messages: Option<usize>,
    pub(super) deliver_intermediate: Option<bool>,
    pub(super) port: Option<u16>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WebhookEntryFile {
    pub(super) secret: Option<String>,
    pub(super) routing: Option<String>,
    pub(super) format: Option<String>,
    pub(super) content_fields: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SkillsConfigFile {
    pub(super) dirs: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ToolsConfigFile {
    pub(super) path: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RetryConfigFile {
    pub(super) max_retries: Option<u32>,
    pub(super) initial_delay_ms: Option<u64>,
    pub(super) max_delay_ms: Option<u64>,
    pub(super) backoff_multiplier: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct IdleConfigFile {
    pub(super) timeout_minutes: Option<u64>,
    pub(super) idle_channel: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CloudConfigFile {
    pub(super) enabled: Option<bool>,
    pub(super) relay_url: Option<String>,
    pub(super) token: Option<String>,
    pub(super) local_port: Option<u16>,
    pub(super) remote_access: Option<bool>,
    pub(super) base_domain: Option<String>,
    pub(super) acme_directory: Option<String>,
    pub(super) acme_root_ca: Option<String>,
    pub(super) pin_service_url: Option<String>,
    pub(super) caa_resolver: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WebSearchConfigFile {
    pub(super) backend: Option<String>,
    pub(super) brave: Option<BraveSearchConfigFile>,
    pub(super) tavily: Option<TavilySearchConfigFile>,
    pub(super) ollama: Option<OllamaSearchConfigFile>,
    pub(super) anthropic: Option<AnthropicSearchConfigFile>,
    pub(super) openai: Option<OpenAiSearchConfigFile>,
    pub(super) gemini: Option<GeminiSearchConfigFile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BraveSearchConfigFile {
    pub(super) api_key: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TavilySearchConfigFile {
    pub(super) api_key: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OllamaSearchConfigFile {
    pub(super) api_key: Option<String>,
    pub(super) base_url: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AnthropicSearchConfigFile {
    pub(super) max_uses: Option<u32>,
    pub(super) allowed_domains: Option<Vec<String>>,
    pub(super) blocked_domains: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OpenAiSearchConfigFile {
    pub(super) search_context_size: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GeminiSearchConfigFile {
    pub(super) exclude_domains: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TracingConfigFile {
    pub(super) log_level: Option<String>,
    pub(super) auto_error_reporting: Option<bool>,
    pub(super) sanitize_content: Option<bool>,
    pub(super) otel_endpoints: Option<Vec<OtelEndpointFile>>,
    pub(super) feedback_endpoint: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OtelEndpointFile {
    pub(super) url: String,
    pub(super) name: Option<String>,
    pub(super) headers: Option<HashMap<String, String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BackgroundModelsFile {
    pub(super) small: Option<ModelAssignment>,
    pub(super) medium: Option<ModelAssignment>,
    pub(super) large: Option<ModelAssignment>,
}
