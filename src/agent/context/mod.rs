//! Context assembly: builds the system prompt from identity files.

mod assembly;
pub(crate) mod loading;
mod prompt;
mod types;

// Re-export all public types at the `context` module level to preserve existing import paths.
pub use types::{
    ContextBreakdown, MemoryContext, ProjectsContext, PromptContext, SkillsContext, StatusLine,
    SubagentsContext,
};

// Re-export assembly functions for use within the agent crate.
pub(super) use assembly::{assemble_system_prompt, compute_context_breakdown};

// Re-export the subagent system content builder for use across the crate.
pub(crate) use prompt::build_subagent_system_content;
