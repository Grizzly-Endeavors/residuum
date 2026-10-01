//! Web Push: notifications to the user's browsers and installed apps.
//!
//! [`PushService`] owns the pieces. The VAPID key pair ([`keys`]) identifies
//! the hub to push services, the devices file ([`devices`]) holds each
//! registered browser's subscription and preferences, [`encrypt`] seals a
//! payload the way RFC 8291 requires, and [`deliver`] sends it and sorts the
//! answer. Triggers call [`PushService::notify`], which returns at once; the
//! HTTP routes that manage devices are in `hub::http::push`. See
//! `docs/systems-usage/notifications.md` and `docs/systems-usage/hub-http.md`.

mod deliver;
mod devices;
mod encrypt;
mod error;
mod keys;
mod service;
pub mod types;

pub use error::PushError;
pub use service::{DEFAULT_CONTACT, PushService, validate_subscription};
pub use types::{
    MAX_BODY_CHARS, PatchPushDeviceRequest, PushDevice, PushDeviceList, PushDeviceResponse,
    PushEvent, PushFailure, PushKeyResponse, PushMessage, PushPayload, PushPreferences,
    PushPreferencesPatch, PushTestResult, PutPushDeviceRequest, Urgency, WebPushSubscription,
    WebPushSubscriptionKeys,
};
