//! Compile-time default values for configuration fields.

/// Default base URL for the Anthropic API.
pub(super) const DEFAULT_ANTHROPIC_URL: &str = "https://api.anthropic.com";

/// Default base URL for a local Ollama instance.
pub(super) const DEFAULT_OLLAMA_URL: &str = "http://localhost:11434";

/// Default base URL for the `OpenAI` API.
pub(super) const DEFAULT_OPENAI_URL: &str = "https://api.openai.com/v1";

/// Default base URL for the Google Gemini API.
pub(super) const DEFAULT_GEMINI_URL: &str = "https://generativelanguage.googleapis.com/v1beta";

/// Default request timeout in seconds.
pub(super) const DEFAULT_TIMEOUT_SECS: u64 = 120;

/// Default gateway bind address.
pub(super) const DEFAULT_GATEWAY_BIND: &str = "127.0.0.1";

/// Default gateway port.
pub(super) const DEFAULT_GATEWAY_PORT: u16 = 7700;

/// Default max tokens for model responses.
pub(super) const DEFAULT_MAX_TOKENS: u32 = 8192;

/// Default observer token threshold before firing.
pub(crate) const DEFAULT_OBSERVER_THRESHOLD: usize = 30_000;

/// Default reflector token threshold before compressing.
pub(crate) const DEFAULT_REFLECTOR_THRESHOLD: usize = 40_000;

/// Default observer cooldown period in seconds before observation fires.
pub(crate) const DEFAULT_OBSERVER_COOLDOWN_SECS: u64 = 120;

/// Default force-observe token threshold (bypasses cooldown).
pub(crate) const DEFAULT_OBSERVER_FORCE_THRESHOLD: usize = 60_000;

/// Default weight for vector similarity in hybrid search merge.
pub(super) const DEFAULT_SEARCH_VECTOR_WEIGHT: f64 = 0.7;

/// Default weight for BM25 text scores in hybrid search merge.
pub(super) const DEFAULT_SEARCH_TEXT_WEIGHT: f64 = 0.3;

/// Default minimum hybrid score threshold.
pub(super) const DEFAULT_SEARCH_MIN_SCORE: f64 = 0.35;

/// Default candidate multiplier for hybrid search over-fetch.
pub(super) const DEFAULT_SEARCH_CANDIDATE_MULTIPLIER: usize = 4;

/// Whether temporal decay is enabled for search scoring by default.
pub(super) const DEFAULT_SEARCH_TEMPORAL_DECAY: bool = false;

/// Default half-life in days for temporal decay scoring.
pub(super) const DEFAULT_SEARCH_TEMPORAL_DECAY_HALF_LIFE_DAYS: f64 = 30.0;

/// Default maximum number of concurrent background tasks.
pub(super) const DEFAULT_MAX_CONCURRENT_BACKGROUND: usize = 3;

/// Default number of days to retain background task transcripts.
pub(super) const DEFAULT_TRANSCRIPT_RETENTION_DAYS: u64 = 30;

/// Whether the agent can modify MCP server configurations by default.
pub(super) const DEFAULT_AGENT_MODIFY_MCP: bool = true;

/// Whether the agent can modify notification channels by default.
pub(super) const DEFAULT_AGENT_MODIFY_CHANNELS: bool = true;

/// Default idle timeout in minutes (30 minutes).
pub(super) const DEFAULT_IDLE_TIMEOUT_MINUTES: u64 = 30;

/// Default relay WebSocket URL.
pub(super) const DEFAULT_CLOUD_RELAY_URL: &str = "wss://agent-residuum.com/tunnel/register";
