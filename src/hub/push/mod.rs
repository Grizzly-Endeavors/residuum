//! Web Push: notifications to the user's browsers and installed apps.
//!
//! [`PushService`] owns the pieces. The VAPID key pair ([`keys`]) identifies
//! the hub to push services, the devices file ([`devices`]) holds each
//! registered browser's subscription and preferences, [`encrypt`] seals a
//! payload the way RFC 8291 requires, and [`deliver`] sends it and sorts the
//! answer. [`triggers`] decides what is worth a push and calls
//! [`PushService::notify`], which returns at once; [`presence`] records which
//! devices have a window in front of the user, so triggers skip them. The
//! HTTP routes that manage devices are in `hub::http::push`. See
//! `docs/systems-usage/notifications.md` and `docs/systems-usage/hub-http.md`.

mod deliver;
mod devices;
mod encrypt;
mod error;
mod keys;
pub mod presence;
mod service;
mod triggers;
pub mod types;

#[cfg(test)]
pub(in crate::hub) use encrypt::browser::Browser;
pub use error::PushError;
pub use presence::{Presence, PresenceConnection};
pub use service::{
    DEFAULT_CONTACT, DeliveryOutcome, DeliveryReport, PushService, validate_subscription,
};
pub(crate) use triggers::{PushTriggers, TriggerInputs};
#[cfg(test)]
pub(in crate::hub) use triggers::{Trigger, TriggerDecision, TriggerInput};
pub use types::{
    MAX_BODY_CHARS, PatchPushDeviceRequest, PushDevice, PushDeviceList, PushDeviceResponse,
    PushEvent, PushFailure, PushKeyResponse, PushMessage, PushPayload, PushPreferences,
    PushPreferencesPatch, PushTestResult, PutPushDeviceRequest, Urgency, WebPushSubscription,
    WebPushSubscriptionKeys,
};
