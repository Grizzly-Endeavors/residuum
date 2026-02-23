//! Agent skills: discovery, activation, and lifecycle management.
//!
//! Skills are opt-in capability modules discovered from `SKILL.md` files.
//! The agent sees a lightweight index in the system prompt and can activate
//! or deactivate skills via tools to load their full instructions.

mod index;
mod parser;
mod state;
mod types;

pub use index::SkillIndex;
pub use state::{SharedSkillState, SkillState};
pub use types::{ActiveSkill, SkillIndexEntry, SkillSource};
