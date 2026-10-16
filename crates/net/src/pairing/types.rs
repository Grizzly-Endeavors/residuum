//! The shapes of the pairing HTTP API. Field names and JSON forms follow
//! `docs/systems-usage/remote-access.md`.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Answer of `GET /api/hub/pairing/state`: what the pairing page needs to know
/// about the browser asking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct PairingStateResponse {
    /// Whether the request arrived through Residuum Cloud. A request made
    /// directly to the machine running Residuum needs no pairing.
    pub remote: bool,
    /// Whether the browser already holds a valid device credential. Always
    /// true for a request that didn't arrive remotely.
    pub paired: bool,
}

/// Body of `POST /api/hub/pairing/requests`.
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct CreatePairingRequestBody {
    /// What to call this device in the list of paired devices.
    pub device_name: String,
}

/// Answer of `POST /api/hub/pairing/requests`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct PairingRequestCreated {
    /// The secret this browser polls with. Only this browser ever sees it.
    pub request_id: String,
    /// The short code shown here and on the approving device, to match them.
    pub code: String,
    /// Seconds until the request expires.
    #[ts(type = "number")]
    pub expires_in_secs: u64,
}

/// Body of `POST /api/hub/pairing/requests/poll`.
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct PollPairingBody {
    /// The `request_id` from creating the request.
    pub request_id: String,
}

/// Where a pairing request stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PairingRequestStatus {
    /// Waiting for a paired device or the local UI to approve it.
    Pending,
    /// Approved; this response also set the device credential.
    Approved,
    /// Refused by the person who saw it.
    Denied,
    /// Expired or unknown. Start a new request.
    Expired,
}

/// Answer of `POST /api/hub/pairing/requests/poll`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct PairingPollResponse {
    /// Where the request stands.
    pub status: PairingRequestStatus,
}

/// Body of `POST /api/hub/pairing/redeem`.
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct RedeemTokenBody {
    /// The single-use token from a pairing link.
    pub token: String,
    /// What to call this device.
    pub device_name: String,
}

/// Body of `POST /api/hub/pairing/recovery`.
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct RecoveryCodeBody {
    /// One of the recovery codes, in any case, with or without dashes.
    pub code: String,
    /// What to call this device.
    pub device_name: String,
}

/// Body of `POST /api/hub/pairing/handoff`, sent on the workbench host.
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct HandoffBody {
    /// The single-use token a paired UI-host browser minted.
    pub token: String,
}

/// Answer of the routes that pair a browser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct PairedResponse {
    /// The name the device was given.
    pub device_name: String,
}

/// A paired browser, as the device list shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DeviceInfo {
    /// Identifies the device for revoking. Not a credential.
    pub id: String,
    /// The name it was paired under.
    pub name: String,
    /// When it was paired.
    #[ts(type = "string")]
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// When it last made a request.
    #[ts(type = "string")]
    pub last_seen: chrono::DateTime<chrono::Utc>,
    /// Whether this is the browser asking.
    pub current: bool,
}

/// A browser waiting to be paired, as the approving device sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PendingPairingInfo {
    /// Identifies the request for approving or refusing. Not the secret the
    /// waiting browser polls with.
    pub id: String,
    /// The code the waiting browser shows. Approve only when it matches.
    pub code: String,
    /// The name the waiting browser gave itself.
    pub device_name: String,
    /// When it asked.
    #[ts(type = "string")]
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// When the request expires.
    #[ts(type = "string")]
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

/// Answer of `GET /api/hub/devices`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DeviceListResponse {
    /// Every paired browser, oldest first.
    pub devices: Vec<DeviceInfo>,
    /// Browsers waiting to be paired, oldest first.
    pub pending: Vec<PendingPairingInfo>,
    /// Unused recovery codes left.
    pub recovery_codes_remaining: u32,
    /// The address a browser reaches this install at, once Residuum Cloud has
    /// announced it. Pairing links point here.
    pub ui_origin: Option<String>,
    /// Whether the request arrived through Residuum Cloud.
    pub remote: bool,
}

/// Answer of `POST /api/hub/remote-access/pair-link`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PairLinkResponse {
    /// The link to open on the device to pair. The token is in its fragment.
    pub link: String,
    /// The same link as an SVG QR code.
    pub qr_svg: String,
    /// Seconds until the link stops working.
    #[ts(type = "number")]
    pub expires_in_secs: u64,
    /// The recovery codes, only when this call created them. Shown once.
    pub recovery_codes: Option<Vec<String>>,
}

/// Answer of `POST /api/hub/devices/recovery-codes`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct RecoveryCodesResponse {
    /// The new codes. Shown once; the previous codes no longer work.
    pub recovery_codes: Vec<String>,
}

/// Answer of `POST /api/hub/devices/workbench-handoff`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct WorkbenchHandoffResponse {
    /// The single-use token to carry to the workbench host in a URL fragment.
    pub token: String,
    /// Seconds until the token stops working.
    #[ts(type = "number")]
    pub expires_in_secs: u64,
}
