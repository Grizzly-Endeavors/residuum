//! Agent sessions: the session registry, runtime, and store that run temporary
//! forks of the main agent. See `docs/systems-usage/background-tasks.md` for the
//! behavior and `README.md` for how the pieces fit together.

pub mod conversation_router;
pub(crate) mod events;
pub(crate) mod listener;
pub mod messaging;
pub mod registry;
pub(crate) mod runtime;
pub(crate) mod session_memory;
pub(crate) mod spawn_context;
pub mod store;
pub mod subagent;
pub mod types;

pub use crate::agent::hop::HopCounter;
pub(crate) use crate::agent::hop::HopLimits;
pub use conversation_router::ConversationRouter;
pub use messaging::{AgentMessenger, DeliveryOutcome, SendError};
pub use registry::{DeliverOutcome, SessionRegistry};
pub use runtime::SessionRuntime;
pub use store::SessionStore;
pub use subagent::{SubAgentResources, build_subagent_resources};
pub use types::{SubAgentBuildConfig, SubAgentConfig};
