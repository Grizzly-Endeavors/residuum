pub use residuum_agent as agent;
pub use residuum_agent::subconscious;
pub use residuum_background as background;
pub use residuum_bus as bus;
pub use residuum_config as config;
pub use residuum_config::agent_keys;
pub use residuum_core::{build_info, diagnostics, process, time, util};
pub use residuum_inference as inference;
pub use residuum_mcp as mcp;
pub use residuum_memory as memory;
pub(crate) use residuum_net::{
    browser_landing, cross_site, remote_control_guard, tunnel, workbench,
};
pub use residuum_net::{pairing, remote_access};
pub use residuum_notify as notify;
pub use residuum_proactivity::{actions, pulse};
pub use residuum_protocol as protocol;
pub use residuum_skills as skills;
pub use residuum_tracing_service as tracing_service;
pub use residuum_workspace as workspace;
pub use residuum_workspace::inbox;

/// The version and commit of this binary. Only this crate reads the build
/// variables; everything else takes the value passed down from here.
pub const BUILD_INFO: build_info::BuildInfo = build_info::BuildInfo {
    version: env!("RESIDUUM_VERSION"),
    commit: option_env!("RESIDUUM_GIT_COMMIT"),
};

pub mod a2a;
pub use residuum_checkpoints as checkpoints;
pub mod daemon;
pub mod gateway;
pub mod hub;
#[cfg(test)]
pub(crate) mod integration;
pub mod interfaces;
pub mod logging;
pub mod setup_wizard;
#[cfg(test)]
pub(crate) mod testing;
pub mod tools;
pub mod wiring;
