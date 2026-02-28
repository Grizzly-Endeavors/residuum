//! Background task infrastructure: spawning, execution, and result delivery.

pub(crate) mod spawn_context;
mod spawner;
pub mod subagent;
pub mod types;

pub use spawner::BackgroundTaskSpawner;
pub use subagent::{SubAgentBuildConfig, SubAgentResources, build_resources};
pub use types::*;
