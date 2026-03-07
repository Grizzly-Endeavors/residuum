//! WebSocket gateway for multi-client access to the agent.

pub mod protocol;
pub(crate) mod types;
mod reload;
mod idle;
mod memory;
mod actions;
mod watcher;
pub mod setup;
mod helpers;
mod ws;
pub(crate) mod web;
mod event_loop;
pub(crate) mod startup;

pub use event_loop::run_gateway;
pub use types::{GatewayExit, ReloadSignal, ServerCommand};
pub use reload::{backup_config, rollback_config};
