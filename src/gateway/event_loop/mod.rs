//! One agent's start-up, HTTP routes, adapters, and main event loop.

mod commands;
mod http;
mod pulse;
mod run_loop;
mod subconscious_hook;
mod turns;

pub(crate) use http::A2aServingDeps;
pub(crate) use http::AdapterSenders;
pub(crate) use http::build_agent_a2a;
pub(crate) use http::run_teams_adapter;
pub(crate) use run_loop::{
    AgentCleanup, AgentControl, AgentExit, AgentStartInputs, agent_span, spawn_agent_loop,
    start_agent,
};
