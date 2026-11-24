#[cfg(test)]
#[path = "../../core/src/testing/crypto.rs"]
mod crypto;

pub mod agent_router;
pub mod auth;
pub mod card;
pub mod client;
pub mod diagnostics_dispatch;
pub mod executor;
pub mod handler;
pub mod keys;
pub mod keys_runtime;
pub mod listener;
#[cfg(test)]
mod listener_tests;
pub mod public_url;
#[cfg(any(test, feature = "test-support"))]
pub mod static_directory;
pub mod task_store;

pub use agent_router::{AgentA2a, AgentA2aState, agent_a2a_router};
pub use auth::{AUTH_CHECK_PATH, Admission, AuthState, CALLER_HEADER, Caller, authorize};
pub use card::{
    AgentCardFile, AgentCardSkillFile, CardError, CardRuntime, CardState, SharedCardState,
    build_agent_card,
};
pub use client::{
    A2aClientHub, AgentSnapshot, AgentSource, AgentStatus, HubError, RemoteTaskTracker,
    SiblingFanout,
};
pub use executor::SessionExecutor;
pub use handler::{ResiduumA2aHandler, resume_in_progress_tasks};
pub use keys::{A2aKeyError, A2aKeyInfo, A2aKeyStore};
pub use keys_runtime::{A2aKeys, SharedA2aKeys};
pub use listener::{A2aListener, StubHandler, agent_handler_router, hub_a2a_app};
#[cfg(any(test, feature = "test-support"))]
pub use static_directory::StaticAgentDirectory;
pub use task_store::{FileTaskStore, SharedTaskStore};

pub use client::spawn_sibling_discovery;
