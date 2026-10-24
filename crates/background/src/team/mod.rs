//! The team slice of the runtime: the router that carries teammate
//! messages between the hub's agents, the [`TeamDirectory`] the runtime
//! reads the hosted agents through, and the agent lifecycle types the hub
//! and the runtime share.

mod directory;
mod router;
mod types;

pub use directory::{DirectoryHandle, TeamDirectory};
pub use router::{
    TeamAddressError, TeamLink, TeamRouter, TeamSendError, TeamTarget, parse_team_address,
};
pub use types::{
    A2aVisibility, Actor, AgentErrorKind, AgentLastError, AgentState, AgentSummary,
    CreateAgentRequest, DeleteOutcome, LifecycleError,
};
