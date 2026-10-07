//! What remote access reports about itself: the shapes of
//! `GET /api/hub/remote-access/status`.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Where remote access stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum RemoteAccessState {
    /// Turned off in the configuration, or Residuum Cloud isn't configured.
    Disabled,
    /// Residuum Cloud is connected over the older tunnel, which the relay can read.
    Legacy,
    /// Waiting for the relay.
    Connecting,
    /// Setting up this instance's certificate account with the pin service.
    Enrolling,
    /// Another instance of this user is already set up; this one must join it first.
    NeedsJoin,
    /// Waiting for the DNS record that lets this instance's account issue certificates.
    WaitingForDns,
    /// Getting a certificate.
    Ordering,
    /// Serving remote access with a certificate.
    Ready,
    /// The relay announced a different identity than this install was set up with.
    Refused,
    /// Something failed; `detail` says what and what to do.
    Error,
}

/// This instance's three host names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RemoteHosts {
    pub ui: String,
    pub workbench: String,
    pub instance: String,
}

/// The certificate in use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CertificateInfo {
    /// When it expires, RFC 3339.
    pub not_after: String,
    /// When renewal starts, RFC 3339.
    pub renews_at: String,
}

/// One account the pin service lists for this user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PinInfo {
    /// The ACME account URL.
    pub account_uri: String,
    /// The instance slug the pin was created for.
    pub slug: String,
    /// Whether this is this instance's own account.
    pub own: bool,
    /// Whether this instance pinned or approved it. An unknown pin is a
    /// certificate account this instance never agreed to.
    pub known: bool,
}

/// `GET /api/hub/remote-access/status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RemoteAccessStatus {
    pub state: RemoteAccessState,
    /// A plain-language explanation of `state`, with what to do when it needs action.
    pub detail: Option<String>,
    /// The user this instance was set up as, once it is.
    pub user: Option<String>,
    /// This instance's slug, once set up.
    pub slug: Option<String>,
    pub hosts: Option<RemoteHosts>,
    pub certificate: Option<CertificateInfo>,
    /// The accounts the pin service lists, when it has been read.
    pub pins: Vec<PinInfo>,
    /// Whether a recovery code is waiting to be saved. The code itself is
    /// in `recovery_code`, for requests made on the machine Residuum runs on.
    pub recovery_code_pending: bool,
    pub recovery_code: Option<String>,
}

impl RemoteAccessStatus {
    /// The status before anything is known.
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
        }
    }

    /// Pins nobody here agreed to.
    #[must_use]
    pub fn unknown_pins(&self) -> Vec<&PinInfo> {
        self.pins.iter().filter(|pin| !pin.known).collect()
    }
}
