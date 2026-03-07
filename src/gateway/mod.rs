//! WebSocket gateway for multi-client access to the agent.

mod actions;
mod event_loop;
mod helpers;
mod idle;
mod memory;
pub mod protocol;
mod reload;
pub mod setup;
pub(crate) mod startup;
pub(crate) mod types;
mod watcher;
pub(crate) mod web;
mod ws;

pub use event_loop::run_gateway;
pub use reload::{backup_config, rollback_config};
pub use types::{GatewayExit, ReloadSignal, ServerCommand};
