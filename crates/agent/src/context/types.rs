//! Context types used in system prompt assembly.

use std::collections::BTreeSet;
use std::ops::Range;

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

/// What a system prompt section is, for the context breakdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SectionKind {
    /// A workspace or team file: SOUL.md, AGENTS.md, BOOTSTRAP.md, USER.md,
    /// the wiki index, or the team roster.
    Identity,
    /// The code-owned HARNESS orientation.
    Harness,
    /// The observation log or the recent context.
    Memory,
    /// The skills index.
    SkillsIndex,
    /// The active skills' instructions.
    ActiveSkills,
}

/// What each part of an assembled model request is, so the context
/// breakdown counts the request a turn sends rather than rebuilding it.
#[derive(Debug, Clone, Default)]
pub(crate) struct RequestLayout {
    /// The system prompt's sections (the request's first message): each
    /// one's kind and the byte range of the text it covers, in order,
    /// together covering the whole text.
    pub(crate) system_sections: Vec<(SectionKind, Range<usize>)>,
    /// Indexes into the request's messages of the stored status lines.
    pub(crate) status_lines: BTreeSet<usize>,
    /// How many of the request's tools are built in; the MCP tools follow
    /// them. Set where the tools are added.
    pub(crate) builtin_tools: usize,
}
