//! Residuum's network edge: the relay tunnel, remote access over it, device
//! pairing, the workbench artifacts listener, and the request helpers they
//! share with the gateway.
//!
//! `docs/systems-usage/remote-access.md` and `docs/systems-usage/secure-tunnel.md`
//! describe the systems.

pub mod browser_landing;
pub mod cross_site;
pub mod pairing;
pub mod remote_access;
pub mod remote_control_guard;
pub mod tunnel;
pub mod workbench;

#[cfg(test)]
#[path = "../../core/src/testing/crypto.rs"]
mod crypto;
