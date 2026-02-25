//! Background task infrastructure: spawning, execution, and result delivery.

mod script;
mod spawner;
pub mod subagent;
pub mod types;

pub use spawner::BackgroundTaskSpawner;
pub use subagent::{SubAgentBuildConfig, SubAgentResources, build_resources};
pub use types::*;
