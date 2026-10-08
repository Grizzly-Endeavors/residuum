//! What remote access reports about itself: the shapes of
//! `GET /api/hub/remote-access/status`.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Where remote access stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum RemoteAccessState {
    /// Residuum Cloud isn't configured on this install.
    Disabled,
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
    /// Whether the person may remove it here: not this instance's own account,
    /// and its instance is no longer in the relay's list of instances.
    pub removable: bool,
}

/// A reset by email that was requested and hasn't taken effect. Once it does,
/// the requesting instance's account is the only pin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PendingResetInfo {
    /// The instance that asked for it.
    pub slug: String,
    /// The certificate account that would replace every pin.
    pub account_uri: String,
    /// Whether it was asked for by this instance.
    pub own: bool,
    /// Whether the link in the email has been confirmed, which starts the hold.
    pub confirmed: bool,
    /// When it takes effect, RFC 3339. Known once it is confirmed.
    pub effective_at: Option<String>,
    /// Whether this instance can cancel it: it is pinned, and didn't ask for it.
    pub cancellable: bool,
}

/// One of the user's instances, as the relay lists it. The relay is not
/// trusted for this: the slug is checked before it is used, and the name is
/// text to display, never markup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct InstanceInfo {
    pub slug: String,
    pub display_name: String,
    /// Whether the user's address currently goes to this instance.
    pub active: bool,
    /// Whether the instance is connected to the relay now.
    pub connected: bool,
}

/// An instance that completed a join with this one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SiblingInfo {
    pub slug: String,
    pub display_name: String,
}

/// Another instance asking this one to approve it, waiting for a decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PendingJoinInfo {
    /// What approving or denying names. Not the id the requester polls with.
    pub id: String,
    /// The six digits that must match the code the requesting instance shows.
    pub code: String,
    /// The slug the request claims.
    pub slug: String,
    /// The name the request claims.
    pub display_name: String,
    /// Whether the relay lists that slug among the user's instances: `None`
    /// when the relay's list isn't known. A hint from the relay, not proof.
    pub in_relay_list: Option<bool>,
    /// When the request expires, RFC 3339.
    pub expires_at: String,
}

/// Where a join this instance started stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum JoinState {
    /// Waiting for the person on the other instance to approve.
    Waiting,
    /// Approved and recorded.
    Approved,
    /// The other instance said no.
    Denied,
    /// It failed or expired; `detail` says why.
    Failed,
}

/// A join this instance started.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct JoinProgress {
    /// The instance being joined.
    pub instance: String,
    pub state: JoinState,
    /// The six digits to compare with the other instance, once the request is sent.
    pub code: Option<String>,
    /// A plain-language explanation of `state`.
    pub detail: Option<String>,
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
    /// The reset by email waiting to take effect, when the pin service reports one.
    pub pending_reset: Option<PendingResetInfo>,
    /// The user's instances as the relay lists them, empty while unknown.
    pub instances: Vec<InstanceInfo>,
    /// Instances that completed a join with this one.
    pub siblings: Vec<SiblingInfo>,
    /// The join this instance started most recently, if any.
    pub join: Option<JoinProgress>,
    /// Instances waiting for this one to approve them.
    pub pending_joins: Vec<PendingJoinInfo>,
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
            pending_reset: None,
            instances: Vec::new(),
            siblings: Vec::new(),
            join: None,
            pending_joins: Vec::new(),
        }
    }

    /// Pins nobody here agreed to.
    #[must_use]
    pub fn unknown_pins(&self) -> Vec<&PinInfo> {
        self.pins.iter().filter(|pin| !pin.known).collect()
    }
}
