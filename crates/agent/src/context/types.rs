//! Context types used in system prompt assembly.

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

/// Bundle of external context injected into the system prompt.
#[derive(Default)]
pub struct PromptContext<'a> {
    pub skills: SkillsContext<'a>,
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
