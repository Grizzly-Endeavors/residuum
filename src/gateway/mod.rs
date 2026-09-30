//! WebSocket gateway for multi-client access to the agent.

mod actions;
mod chat_adapters;
pub(crate) mod cross_site;
pub(crate) mod event_loop;
pub mod file_server;
pub(crate) mod helpers;
mod idle;
pub(crate) mod last_known_good;
mod memory;
pub(crate) mod post_turn;
pub mod protocol;
mod reload;
pub(crate) mod remote_control_guard;
pub(crate) mod sessions;
pub(crate) mod startup;
pub(crate) mod types;
pub(crate) mod watcher;
pub(crate) mod web;
mod ws;

pub use last_known_good::exists as has_last_known_good;
pub use last_known_good::hub::exists as has_hub_last_known_good;
pub use last_known_good::hub::load as load_hub_last_known_good;
pub use types::{GatewayExit, ReloadSignal, ServerCommand};
