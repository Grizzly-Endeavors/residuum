//! Scheduled actions: lightweight one-off scheduling for the agent.
//!
//! Actions are fire-once entries with an ISO datetime trigger. When the time arrives,
//! the action either spawns a sub-agent or injects a prompt into the main agent as
//! a wake turn. Once fired, the action is removed from the store.

pub mod store;
pub mod types;
