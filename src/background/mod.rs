//! Background task infrastructure: spawning, execution, and result delivery.

mod script;
mod spawner;
mod subagent;
pub mod types;

pub use spawner::BackgroundTaskSpawner;
pub use types::*;
