//! WebSocket gateway for multi-client access to the agent.

mod actions;
mod chat_adapters;
pub(crate) mod event_loop;
pub mod file_server;
mod idle;
mod memory;
pub(crate) mod post_turn;
pub(crate) mod queued_messages;
mod reload;
pub(crate) mod sessions;
pub(crate) mod startup;
pub mod turn_journal;
pub(crate) mod types;
pub(crate) mod web;
pub mod websocket;
mod ws;

pub use web::teams_setup;
