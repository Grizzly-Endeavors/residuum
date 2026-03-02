//! Google A2A (Agent-to-Agent) protocol support.
//!
//! Implements the A2A specification for agent interoperability:
//!
//! **Server-side** — allows Residuum to be discovered and invoked by other
//! A2A-compatible agents:
//! - Agent Card served at `GET /.well-known/agent.json`
//! - JSON-RPC 2.0 endpoint at `POST /a2a`
//! - Methods: `tasks/send`, `tasks/get`, `tasks/cancel`
//!
//! **Client-side** — allows Residuum to discover and delegate tasks to
//! remote A2A agents via tools (`a2a_discover`, `a2a_delegate`, `a2a_list_agents`).

pub(crate) mod client;
pub(crate) mod registry;
pub(crate) mod server;
pub mod types;
