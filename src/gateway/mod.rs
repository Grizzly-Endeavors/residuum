//! WebSocket gateway for multi-client access to the agent.

mod actions;
pub(crate) mod cross_site;
pub(crate) mod event_loop;
pub mod file_server;
pub(crate) mod helpers;
mod idle;
pub(crate) mod last_known_good;
mod memory;
pub mod protocol;
mod reload;
pub(crate) mod remote_control_guard;
pub(crate) mod sessions;
pub mod setup;
pub(crate) mod startup;
pub(crate) mod types;
mod watcher;
pub(crate) mod web;
mod ws;

pub use event_loop::{run_gateway, run_gateway_with_config};
pub use last_known_good::exists as has_last_known_good;
pub use types::{GatewayExit, ReloadSignal, ServerCommand};
