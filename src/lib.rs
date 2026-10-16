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
pub use residuum_skills as skills;
pub use residuum_tracing_service as tracing_service;

/// The version and commit of this binary. Only this crate reads the build
/// variables; everything else takes the value passed down from here.
pub const BUILD_INFO: build_info::BuildInfo = build_info::BuildInfo {
    version: env!("RESIDUUM_VERSION"),
    commit: option_env!("RESIDUUM_GIT_COMMIT"),
};

pub mod a2a;
pub mod actions;
pub mod agent;
pub mod background;
pub mod bus;
pub mod checkpoints;
pub mod client_context;
pub mod daemon;
pub(crate) mod features;
pub mod gateway;
pub mod hub;
pub mod inbox;
pub mod interfaces;
pub mod logging;
pub mod notify;
pub mod pulse;
pub mod setup_wizard;
pub mod subconscious;
#[cfg(test)]
pub(crate) mod testing;
pub mod tools;
pub mod update;
pub mod workspace;
