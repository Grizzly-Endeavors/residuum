//! The shapes of the pairing HTTP API. Field names and JSON forms follow
//! `docs/systems-usage/remote-access.md`.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct PairingStateResponse {
    pub remote: bool,
    pub paired: bool,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct CreatePairingRequestBody {
    pub device_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct PairingRequestCreated {
    pub request_id: String,
    pub code: String,
    #[ts(type = "number")]
    pub expires_in_secs: u64,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct PollPairingBody {
    pub request_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PairingRequestStatus {
    Pending,
    Approved,
    Denied,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct PairingPollResponse {
    pub status: PairingRequestStatus,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct RedeemTokenBody {
    pub token: String,
    pub device_name: String,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct RecoveryCodeBody {
    pub code: String,
    pub device_name: String,
}

/// Body of `POST /api/hub/pairing/handoff`, sent on the workbench host.
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct HandoffBody {
    pub token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct PairedResponse {
    pub device_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    #[ts(type = "string")]
    pub created_at: chrono::DateTime<chrono::Utc>,
    #[ts(type = "string")]
    pub last_seen: chrono::DateTime<chrono::Utc>,
    pub current: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PendingPairingInfo {
    pub id: String,
    pub code: String,
    pub device_name: String,
    #[ts(type = "string")]
    pub created_at: chrono::DateTime<chrono::Utc>,
    #[ts(type = "string")]
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DeviceListResponse {
    pub devices: Vec<DeviceInfo>,
    pub pending: Vec<PendingPairingInfo>,
    pub recovery_codes_remaining: u32,
    pub ui_origin: Option<String>,
    pub remote: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PairLinkResponse {
    pub link: String,
    pub qr_svg: String,
    #[ts(type = "number")]
    pub expires_in_secs: u64,
    pub recovery_codes: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct RecoveryCodesResponse {
    pub recovery_codes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct WorkbenchHandoffResponse {
    pub token: String,
    #[ts(type = "number")]
    pub expires_in_secs: u64,
}
