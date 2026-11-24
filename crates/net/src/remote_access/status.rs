//! What remote access reports about itself: the shapes of
//! `GET /api/hub/remote-access/status`.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum RemoteAccessState {
    Disabled,
    Connecting,
    Enrolling,
    NeedsJoin,
    WaitingForDns,
    Ordering,
    Ready,
    Refused,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RemoteHosts {
    pub ui: String,
    pub workbench: String,
    pub instance: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CertificateInfo {
    pub not_after: String,
    pub renews_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PinInfo {
    pub account_uri: String,
    pub slug: String,
    pub own: bool,
    pub known: bool,
    pub removable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PendingResetInfo {
    pub slug: String,
    pub account_uri: String,
    pub own: bool,
    pub confirmed: bool,
    /// When it takes effect, RFC 3339. Known once it is confirmed.
    pub effective_at: Option<String>,
    pub cancellable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct InstanceInfo {
    pub slug: String,
    pub display_name: String,
    pub active: bool,
    pub connected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SiblingInfo {
    pub slug: String,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PendingJoinInfo {
    pub id: String,
    pub code: String,
    pub slug: String,
    pub display_name: String,
    pub in_relay_list: Option<bool>,
    /// When the request expires, RFC 3339.
    pub expires_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum JoinState {
    Waiting,
    Approved,
    Denied,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct JoinProgress {
    pub instance: String,
    pub state: JoinState,
    pub code: Option<String>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RemoteAccessStatus {
    pub state: RemoteAccessState,
    pub detail: Option<String>,
    pub user: Option<String>,
    pub slug: Option<String>,
    pub hosts: Option<RemoteHosts>,
    pub certificate: Option<CertificateInfo>,
    pub pins: Vec<PinInfo>,
    pub(crate) recovery_code_pending: bool,
    pub recovery_code: Option<String>,
    pub pending_reset: Option<PendingResetInfo>,
    pub instances: Vec<InstanceInfo>,
    pub siblings: Vec<SiblingInfo>,
    pub join: Option<JoinProgress>,
    pub pending_joins: Vec<PendingJoinInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub(crate) relay_origin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub(crate) checked_at: Option<String>,
}

impl RemoteAccessStatus {
    #[must_use]
    pub fn new(state: RemoteAccessState) -> Self {
        Self {
            state,
            detail: None,
            user: None,
            slug: None,
            hosts: None,
            certificate: None,
            pins: Vec::new(),
            recovery_code_pending: false,
            recovery_code: None,
            pending_reset: None,
            instances: Vec::new(),
            siblings: Vec::new(),
            join: None,
            pending_joins: Vec::new(),
            relay_origin: None,
            checked_at: None,
        }
    }

    #[must_use]
    pub fn unknown_pins(&self) -> Vec<&PinInfo> {
        self.pins.iter().filter(|pin| !pin.known).collect()
    }
}
