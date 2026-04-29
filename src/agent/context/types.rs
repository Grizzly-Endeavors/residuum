//! Context types used in system prompt assembly.

use chrono::NaiveDateTime;

/// Ephemeral context injected before the last user message in each LLM call.
pub struct StatusLine {
    /// Current local time.
    pub now: NaiveDateTime,
    /// When the previous user message was sent (if any).
    pub last_message_at: Option<NaiveDateTime>,
    /// Which interface this message arrived from (e.g. `"websocket"`, `"discord"`).
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

/// Projects-related context injected into the system prompt.
#[derive(Default)]
pub struct ProjectsContext<'a> {
    /// Formatted project index (always present after bootstrap).
    pub index: Option<&'a str>,
    /// Formatted active project context (only when a project is active).
    pub active_context: Option<&'a str>,
}

/// Skills-related context injected into the system prompt.
#[derive(Default)]
pub struct SkillsContext<'a> {
    /// Formatted skills index XML (available skills listing).
    pub index: Option<&'a str>,
    /// Formatted active skill instructions XML.
    pub active_instructions: Option<&'a str>,
}

/// Subagent-preset-related context injected into the system prompt.
#[derive(Default)]
pub struct SubagentsContext<'a> {
    /// Formatted subagent presets index XML (available presets listing).
    pub index: Option<&'a str>,
}

/// Bundle of external context injected into the system prompt.
///
/// Groups projects, skills, and subagents context into a single struct to
/// reduce argument count on functions that thread all three through.
#[derive(Default)]
pub struct PromptContext<'a> {
    pub projects: ProjectsContext<'a>,
    pub skills: SkillsContext<'a>,
    pub subagents: SubagentsContext<'a>,
}

/// A per-section breakdown of the agent's approximate token usage.
pub struct ContextBreakdown {
    /// Estimated tokens from stable identity files (soul, agents, env, user, memory).
    pub identity_tokens: usize,
    /// Estimated tokens from the memory pipeline (observation log + recent context).
    pub memory_pipeline_tokens: usize,
    /// Estimated tokens from the subagents preset index.
    pub subagents_index_tokens: usize,
    /// Estimated tokens from the projects index.
    pub projects_index_tokens: usize,
    /// Estimated tokens from the active project context (0 if none active).
    pub active_project_tokens: usize,
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
