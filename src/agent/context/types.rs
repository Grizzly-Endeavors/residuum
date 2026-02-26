//! Context types used in system prompt assembly.

use chrono::NaiveDateTime;

/// Ephemeral context injected before the last user message in each LLM call.
pub struct StatusLine {
    /// Current local time.
    pub now: NaiveDateTime,
    /// When the previous user message was sent (if any).
    pub last_message_at: Option<NaiveDateTime>,
    /// Which channel this message arrived from (e.g. `"websocket"`, `"discord"`).
    pub message_source: Option<String>,
    /// Number of unread inbox items (0 → tag omitted).
    pub unread_inbox_count: usize,
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
pub struct ProjectsContext<'a> {
    /// Formatted project index (always present after bootstrap).
    pub index: Option<&'a str>,
    /// Formatted active project context (only when a project is active).
    pub active_context: Option<&'a str>,
}

impl ProjectsContext<'_> {
    /// Empty projects context (no index, no active project).
    #[must_use]
    pub fn none() -> ProjectsContext<'static> {
        ProjectsContext {
            index: None,
            active_context: None,
        }
    }
}

/// Skills-related context injected into the system prompt.
pub struct SkillsContext<'a> {
    /// Formatted skills index XML (available skills listing).
    pub index: Option<&'a str>,
    /// Formatted active skill instructions XML.
    pub active_instructions: Option<&'a str>,
}

impl SkillsContext<'_> {
    /// Empty skills context (no index, no active skills).
    #[must_use]
    pub fn none() -> SkillsContext<'static> {
        SkillsContext {
            index: None,
            active_instructions: None,
        }
    }
}

/// Subagent-preset-related context injected into the system prompt.
pub struct SubagentsContext<'a> {
    /// Formatted subagent presets index XML (available presets listing).
    pub index: Option<&'a str>,
}

impl SubagentsContext<'_> {
    /// Empty subagents context (no index).
    #[must_use]
    pub fn none() -> SubagentsContext<'static> {
        SubagentsContext { index: None }
    }
}

/// Bundle of external context injected into the system prompt.
///
/// Groups projects, skills, and subagents context into a single struct to
/// reduce argument count on functions that thread all three through.
pub struct PromptContext<'a> {
    pub projects: ProjectsContext<'a>,
    pub skills: SkillsContext<'a>,
    pub subagents: SubagentsContext<'a>,
}

impl PromptContext<'_> {
    /// Empty prompt context (no projects, skills, or subagents).
    #[must_use]
    pub fn none() -> PromptContext<'static> {
        PromptContext {
            projects: ProjectsContext::none(),
            skills: SkillsContext::none(),
            subagents: SubagentsContext::none(),
        }
    }
}

/// A snapshot of the agent's approximate token usage.
pub struct ContextSummary {
    /// Estimated tokens in the system prompt (identity + memory; no projects/skills).
    pub system_tokens: usize,
    /// Estimated tokens across the in-memory recent message history.
    pub history_tokens: usize,
    /// Number of messages in the recent history.
    pub history_count: usize,
}
