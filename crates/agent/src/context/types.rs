//! Context types used in system prompt assembly.

use chrono::NaiveDateTime;

/// Ephemeral context injected before the last user message in each LLM call.
pub struct StatusLine {
    /// Current local time.
    pub now: NaiveDateTime,
    /// When the previous user message was sent (if any).
    pub last_message_at: Option<NaiveDateTime>,
    /// Which interface this message arrived from (e.g. `"ws"`, `"discord"`).
    pub message_source: Option<String>,
}

/// Memory-related context injected into the system prompt.
///
/// Groups observation log and recent narrative context to avoid parameter
/// explosion on `assemble_system_prompt` and `execute_turn`.
pub struct MemoryContext<'a> {
    /// Formatted observation log content (if present).
    pub observations: Option<&'a str>,
    /// Narrative summary from the most recent observation (if present).
    pub recent_context: Option<&'a str>,
}

/// Skills-related context injected into the system prompt.
#[derive(Default)]
pub struct SkillsContext<'a> {
    /// Formatted skills index XML (available skills listing).
    pub index: Option<&'a str>,
    /// Formatted active skill instructions XML.
    pub active_instructions: Option<&'a str>,
}

/// Which chat interfaces post the text the agent writes before a tool call
/// (each interface's `deliver_intermediate` setting), so the `HARNESS`
/// section tells the agent what reaches each chat. An interface that isn't
/// configured counts as off.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IntermediateDelivery {
    pub telegram: bool,
    pub discord: bool,
    pub teams: bool,
}

impl IntermediateDelivery {
    /// Read each chat interface's `deliver_intermediate` setting.
    #[must_use]
    pub fn from_config(cfg: &residuum_config::Config) -> Self {
        Self {
            telegram: cfg
                .telegram
                .as_ref()
                .is_some_and(|t| t.deliver_intermediate),
            discord: cfg.discord.as_ref().is_some_and(|d| d.deliver_intermediate),
            teams: cfg.teams.as_ref().is_some_and(|t| t.deliver_intermediate),
        }
    }
}

/// Bundle of external context injected into the system prompt.
#[derive(Default)]
pub struct PromptContext<'a> {
    pub skills: SkillsContext<'a>,
    /// What the chat interfaces post, for the `HARNESS` section.
    pub intermediate_delivery: IntermediateDelivery,
}

/// A per-section breakdown of the agent's approximate token usage.
pub struct ContextBreakdown {
    /// Estimated tokens from stable identity files (soul, agents, env, user, memory).
    pub identity_tokens: usize,
    /// Estimated tokens from the memory pipeline (observation log + recent context).
    pub memory_pipeline_tokens: usize,
    /// Estimated tokens from the skills index.
    pub skills_index_tokens: usize,
    /// Estimated tokens from active skill instructions (0 if none active).
    pub active_skills_tokens: usize,
    /// Estimated tokens from built-in system tool definitions.
    pub system_tool_tokens: usize,
    /// Estimated tokens from MCP tool definitions.
    pub mcp_tool_tokens: usize,
    /// Estimated tokens across the in-memory recent message history.
    pub history_tokens: usize,
    /// Number of messages in the recent history.
    pub history_count: usize,
}
