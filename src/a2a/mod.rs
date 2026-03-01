//! Google A2A (Agent-to-Agent) protocol support.
//!
//! Implements the A2A specification for agent interoperability, allowing
//! Residuum to be discovered and invoked by other A2A-compatible agents.
//!
//! - Agent Card served at `GET /.well-known/agent.json`
//! - JSON-RPC 2.0 endpoint at `POST /a2a`
//! - Methods: `tasks/send`, `tasks/get`, `tasks/cancel`

pub(crate) mod server;
pub mod types;
