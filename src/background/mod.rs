//! Agent sessions: the session registry, runtime, and store that replace the
//! old fire-and-forget background task spawner. See `docs/design/agent-sessions.md`
//! for the systems-level design and `README.md` for how the pieces fit together.

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
