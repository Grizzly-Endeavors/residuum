//! Context assembly: builds the system prompt from identity files.

mod assembly;
mod breakdown;
pub mod loading;
mod prompt;
mod status;
mod types;

// Re-export all public types at the `context` module level to preserve existing import paths.
pub use breakdown::{ContextBreakdown, ContextSnapshot, load_context_snapshot};
pub use status::StatusClock;
pub use types::{MemoryContext, PromptContext, SkillsContext};

// Re-export assembly functions for use within the agent crate.
pub(super) use assembly::assemble_system_prompt;
pub(super) use breakdown::{compute_context_breakdown, save_context_snapshot};
pub(crate) use types::RequestLayout;
