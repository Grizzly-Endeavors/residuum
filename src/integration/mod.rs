//! Integration tests that need code from more than one crate's layer of the
//! workspace: the hub with the gateway's launcher, the hub's file routes with
//! the agents' file tools, the A2A server with the tools' session tools. They
//! are root unit tests, which can reach every module.
//!
//! Each module's leading `//! covers:` line names the packages whose behavior
//! it tests, as a `tests/it` module's does; the pre-push hook runs a module
//! when a push touches one of those packages or the module's own files.

mod hub_agent_tools;
mod hub_host;
