//! Agent sessions: the session registry, runtime, and store that run temporary
//! forks of the main agent. See `docs/systems-usage/background-tasks.md` for the
//! behavior and `README.md` for how the pieces fit together.

#[cfg(test)]
#[path = "../../core/src/testing/crypto.rs"]
mod crypto;

pub mod conversation_router;
pub(crate) mod listener;
pub mod messaging;
pub(crate) mod owner_session;
pub mod registry;
pub(crate) mod runtime;
pub(crate) mod session_memory;
pub mod session_tools;
pub(crate) mod spawn_context;
pub mod store;
pub mod subagent;
pub mod team;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;
pub mod types;

pub use conversation_router::ConversationRouter;
pub use listener::spawn_listener;
pub use messaging::{AgentMessenger, DeliveryOutcome, SendError};
pub use owner_session::{OwnerSessionStart, load_main_conversation};
pub use registry::{DeliverOutcome, SessionRegistry};
pub use residuum_agent::hop::HopCounter;
pub use runtime::{IdleTimeouts, SessionRuntime, SessionRuntimeHandles, SessionSpawnRequest};
pub use session_memory::SessionMemoryEnv;
pub use spawn_context::{BackgroundTierActiveIndex, SpawnContext};
pub use store::SessionStore;
pub use subagent::{SubAgentResources, build_subagent_resources};
pub use types::{SubAgentBuildConfig, SubAgentConfig};
