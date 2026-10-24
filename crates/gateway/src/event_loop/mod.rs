//! One agent's start-up, HTTP routes, adapters, and main event loop.

mod commands;
mod http;
mod pulse;
mod pulse_fire;
mod run_loop;
mod subconscious_hook;
mod turns;

pub(crate) use http::A2aServingDeps;
pub(crate) use http::build_agent_a2a;
pub(crate) use http::run_teams_adapter;
pub(crate) use run_loop::{agent_span_layer, start_agent};
