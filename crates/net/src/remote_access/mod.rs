pub(crate) mod acme;
pub(crate) mod caa;
pub(crate) mod challenge;
pub(crate) mod engine;
pub(crate) mod engine_limit;
#[cfg(test)]
mod engine_tests;
#[cfg(test)]
mod fake_pin_service;
pub(crate) mod identity;
pub(crate) mod jws;
pub mod manager;
#[cfg(test)]
pub(crate) mod pebble_support;
pub(crate) mod pins;
pub(crate) mod proxy;
pub mod siblings;
pub mod slot;
pub mod status;
pub(crate) mod store;
#[cfg(test)]
mod system_tests;
pub(crate) mod tls;
pub(crate) mod types;

pub use siblings::discovery::SecureDiscovery;

pub const TEAMS_MESSAGES_PATH: &str = "/api/teams/messages";
pub const A2A_AGENTS_PATH_PREFIX: &str = "/agents";
