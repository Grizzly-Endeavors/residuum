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

pub use cookies::cookie_header;
pub use error::PairingError;
pub(crate) use gate::HANDOFF_PAGE_PATH;
pub use gate::{AuthenticatedDevice, GateState, PAIRING_PAGE_PATH, device_gate};
pub use remote::{RemoteContext, RemoteTransport, remote_context};
pub use service::{CreatedRequest, DevicePairing, MintedPairToken};
pub use state::{AuthHit, Issued, PollOutcome};
pub use store::Identity;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    Ui,
    Workbench,
}
