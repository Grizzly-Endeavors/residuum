//! Background task infrastructure: spawning, execution, and result delivery.

pub mod bridge;
pub(crate) mod spawn_context;
mod spawner;
pub mod subagent;
pub mod types;

pub use spawner::BackgroundTaskSpawner;
pub use subagent::{SubAgentBuildConfig, SubAgentResources, build_resources};
pub use types::{
    ActiveTaskInfo, BackgroundResult, Execution, SubAgentConfig, format_background_result,
};
