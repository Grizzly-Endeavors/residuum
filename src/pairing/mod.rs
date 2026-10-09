//! Device pairing: who may reach this install through Residuum Cloud.
//!
//! A request that arrives through the relay is the owner's only if it carries
//! the credential of a paired device. Pairing is how a browser gets one:
//! with a link made on the machine Residuum runs on, with a code approved from
//! a device that is already paired, or with a recovery code. Requests made
//! directly to the machine are not remote and need nothing.
//!
//! - [`DevicePairing`] holds the paired devices (as hashes), the pending
//!   requests and tokens, the rate limits and the identity Residuum Cloud
//!   announced, and saves them to `hub/remote-access.json`.
//! - [`device_gate`] is the middleware in front of the main and workbench
//!   routers.
//! - [`remote`] decides which requests are remote, and holds the cross-site
//!   rule they must pass.
//! - The pairing API's handlers live with the hub's other routes, in
//!   `hub::http::pairing`.
//!
//! `docs/systems-usage/remote-access.md` describes the whole system.

mod cookies;
mod error;
mod gate;
mod limits;
pub mod qr;
pub(crate) mod remote;
mod secrets;
mod service;
mod state;
mod store;
pub mod types;

pub(crate) use cookies::cookie_header;
pub use error::PairingError;
pub use gate::DEVICE_REQUIRED_CODE;
pub(crate) use gate::{
    AuthenticatedDevice, GateState, HANDOFF_PAGE_PATH, PAIRING_PAGE_PATH, device_gate,
};
pub use service::DevicePairing;
pub(crate) use state::{Issued, PollOutcome};
pub use store::Identity;

/// Which host a credential is for. The UI host and the workbench host are
/// different origins, so each has its own credential.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Surface {
    /// The web UI and API.
    Ui,
    /// Workbench artifacts.
    Workbench,
}
