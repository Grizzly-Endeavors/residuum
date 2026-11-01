//! Context assembly: builds the system prompt from identity files.

mod assembly;
pub mod loading;
mod prompt;
mod status;
mod types;

// Re-export all public types at the `context` module level to preserve existing import paths.
pub use status::StatusClock;
pub use types::{
    ContextBreakdown, IntermediateDelivery, MemoryContext, PromptContext, SkillsContext,
};

// Re-export assembly functions for use within the agent crate.
pub(super) use assembly::{assemble_system_prompt, compute_context_breakdown};
