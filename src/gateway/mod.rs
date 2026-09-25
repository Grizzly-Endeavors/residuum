//! WebSocket gateway for multi-client access to the agent.

mod actions;
pub(crate) mod cross_site;
pub(crate) mod event_loop;
pub mod file_server;
pub(crate) mod helpers;
mod idle;
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

pub use event_loop::run_gateway;
pub use types::{GatewayExit, ReloadSignal, ServerCommand};
