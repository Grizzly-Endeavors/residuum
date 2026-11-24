pub const WEB_UI_ENDPOINT: &str = "ws";

pub const TEAMS_ENDPOINT: &str = "teams";

pub(super) const DEFAULT_ANTHROPIC_URL: &str = "https://api.anthropic.com";

pub(super) const DEFAULT_OLLAMA_URL: &str = "http://localhost:11434";

pub(super) const DEFAULT_TYPESAFE_URL: &str = "https://api.typesafe.ai";

pub(super) const DEFAULT_TYPESAFE_MODEL: &str = "jev-latest";

pub(super) const TYPESAFE_API_KEY_ENV: &str = "TYPESAFE_API_KEY";

pub(super) const DEFAULT_OPENAI_URL: &str = "https://api.openai.com/v1";

pub(super) const DEFAULT_GEMINI_URL: &str = "https://generativelanguage.googleapis.com/v1beta";

pub(super) const DEFAULT_FIREWORKS_URL: &str = "https://api.fireworks.ai/inference/v1";

pub(super) const DEFAULT_TIMEOUT_SECS: u64 = 120;

pub(super) const DEFAULT_GATEWAY_BIND: &str = "127.0.0.1";

pub const DEFAULT_GATEWAY_PORT: u16 = 7700;

pub(super) const DEFAULT_FEEDBACK_ENDPOINT: &str = "https://agent-residuum.com";

pub(super) const DEFAULT_MAX_TOKENS: u32 = 16_384;

pub(crate) const DEFAULT_PULSE_ENABLED: bool = true;

pub const DEFAULT_OBSERVER_THRESHOLD: usize = 30_000;

pub const DEFAULT_REFLECTOR_THRESHOLD: usize = 22_000;

pub const DEFAULT_OBSERVER_COOLDOWN_SECS: u64 = 120;

pub const DEFAULT_OBSERVER_FORCE_THRESHOLD: usize = 60_000;

pub const DEFAULT_SUBCONSCIOUS_EVERY_N_ITERATIONS: usize = 3;

pub const DEFAULT_SUBCONSCIOUS_MAX_TRANSCRIPT_TOKENS: usize = 12_000;

pub(crate) const DEFAULT_LEARNING_COOLDOWN_MINUTES: u64 = 240;

pub(crate) const DEFAULT_LEARNING_NUDGE_AFTER_TURNS: u32 = 0;

pub(super) const DEFAULT_SEARCH_VECTOR_WEIGHT: f64 = 0.7;

pub(super) const DEFAULT_SEARCH_TEXT_WEIGHT: f64 = 0.3;

pub(super) const DEFAULT_SEARCH_MIN_SCORE: f64 = 0.35;

pub(super) const DEFAULT_SEARCH_CANDIDATE_MULTIPLIER: usize = 4;

pub(super) const DEFAULT_SEARCH_TEMPORAL_DECAY: bool = false;

pub(super) const DEFAULT_SEARCH_TEMPORAL_DECAY_HALF_LIFE_DAYS: f64 = 30.0;

pub(super) const DEFAULT_MAX_CONCURRENT_BACKGROUND: usize = 3;

pub(super) const DEFAULT_AGENT_MODIFY_MCP: bool = true;

pub(super) const DEFAULT_AGENT_MODIFY_CHANNELS: bool = true;

pub(super) const DEFAULT_IDLE_TIMEOUT_MINUTES: u64 = 30;

pub(super) const DEFAULT_IDLE_TIMEOUT_SCHEDULED_MINUTES: u64 = 2;

pub(super) const DEFAULT_IDLE_TIMEOUT_SPAWNED_MINUTES: u64 = 10;

pub(super) const DEFAULT_IDLE_TIMEOUT_EXTERNAL_MINUTES: u64 = 30;

pub(super) const DEFAULT_IDLE_TIMEOUT_ARTIFACT_MINUTES: u64 = 10;

pub(super) const DEFAULT_EPISODE_SKIP_TOKEN_FLOOR: usize = 2_000;

pub(super) const DEFAULT_SUBAGENT_DEPTH_CAP: u32 = 3;

pub(super) const DEFAULT_HOP_SOFT_LIMIT: u32 = 8;

pub(super) const DEFAULT_HOP_HARD_LIMIT: u32 = 32;

pub(super) const DEFAULT_TEAMS_CONTEXT_MESSAGES: usize = 20;

pub(super) const DEFAULT_DISCORD_CONTEXT_MESSAGES: usize = 20;

pub(super) const DEFAULT_TELEGRAM_CONTEXT_MESSAGES: usize = 20;

pub const DEFAULT_TEAMS_PORT: u16 = 7701;

pub const DEFAULT_A2A_PORT: u16 = 7702;

pub const DEFAULT_CLOUD_RELAY_URL: &str = "wss://agent-residuum.com/tunnel/v2/register";

pub(super) const DEFAULT_REPEAT_CALL_STEER_AFTER: u32 = 3;

pub(super) const DEFAULT_REPEAT_CALL_STOP_AFTER: u32 = 6;
