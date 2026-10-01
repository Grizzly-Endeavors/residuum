//! The shapes of the Web Push HTTP API and of the payload the service worker
//! receives. Field names and JSON forms follow
//! `docs/systems-usage/hub-http.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Why a notification is sent. The first four are the events a device can
/// turn on or off; `test` is what the test button sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PushEvent {
    /// An agent filed an item in the user inbox.
    InboxItem,
    /// An agent entered the failed state.
    AgentFailed,
    /// A task sent to a remote agent has been unreachable past its notice
    /// threshold.
    OutboundUnreachable,
    /// A main turn the user didn't start produced a reply while no client was
    /// connected.
    ReplyWhileAway,
    /// A notification the user asked for from the Notifications section.
    Test,
}

impl PushEvent {
    /// The events a device can turn on or off, with each one's name in a
    /// preferences object.
    const SWITCHABLE: [(Self, &'static str); 4] = [
        (Self::InboxItem, "inbox_item"),
        (Self::AgentFailed, "agent_failed"),
        (Self::OutboundUnreachable, "outbound_unreachable"),
        (Self::ReplyWhileAway, "reply_while_away"),
    ];

    /// How urgently the push service should hand the message to the device.
    #[must_use]
    pub fn urgency(self) -> Urgency {
        match self {
            Self::AgentFailed => Urgency::High,
            Self::InboxItem | Self::OutboundUnreachable | Self::ReplyWhileAway | Self::Test => {
                Urgency::Normal
            }
        }
    }

    /// How long the push service keeps the message for a device that is
    /// offline. A reply is stale within the hour, an unreachable task within
    /// hours, and an item or failure within a day.
    #[must_use]
    pub fn ttl(self) -> Duration {
        let hours = match self {
            Self::InboxItem | Self::AgentFailed => 24,
            Self::OutboundUnreachable => 6,
            Self::ReplyWhileAway | Self::Test => 1,
        };
        Duration::from_secs(hours * 3600)
    }
}

/// The `Urgency` header of a push message (RFC 8030 section 5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Urgency {
    /// Delivered immediately, even to a sleeping device.
    High,
    /// The push service's default.
    Normal,
}

impl Urgency {
    /// The header's value.
    #[must_use]
    pub(super) fn header_value(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Normal => "normal",
        }
    }
}

/// The most characters a payload's body holds; longer text is cut with an
/// ellipsis so a notification stays readable on a lock screen.
pub const MAX_BODY_CHARS: usize = 120;

/// The JSON a device's service worker decrypts from a push message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PushPayload {
    /// The payload format version, always 1.
    #[ts(type = "1")]
    pub v: u8,
    /// Why the notification was sent.
    pub event: PushEvent,
    /// The agent the notification is about, or an empty string when it is
    /// about none (the test notification).
    pub agent: String,
    /// The notification's title.
    pub title: String,
    /// Plain text, at most 120 characters.
    pub body: String,
    /// The app path a click opens, such as `/inbox?item=scout:note-1`.
    pub target: String,
    /// Notifications with the same tag replace each other.
    pub tag: String,
    /// The user's total unread inbox items, for the app badge.
    pub badge: u32,
}

impl PushPayload {
    /// A payload of format version 1. `body` is cut to [`MAX_BODY_CHARS`]
    /// characters.
    #[must_use]
    pub fn new(
        event: PushEvent,
        agent: impl Into<String>,
        title: impl Into<String>,
        body: &str,
        target: impl Into<String>,
        tag: impl Into<String>,
        badge: u32,
    ) -> Self {
        Self {
            v: 1,
            event,
            agent: agent.into(),
            title: title.into(),
            body: truncate_chars(body, MAX_BODY_CHARS),
            target: target.into(),
            tag: tag.into(),
            badge,
        }
    }
}

/// `text` cut to at most `max` characters, ending in an ellipsis when it was
/// cut.
fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let kept: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", kept.trim_end())
}

/// A payload with the delivery terms its event carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushMessage {
    /// What the device decrypts.
    pub payload: PushPayload,
    /// The `Urgency` header.
    pub urgency: Urgency,
    /// The `TTL` header.
    pub ttl: Duration,
}

impl PushMessage {
    /// A message with the urgency and lifetime its event calls for.
    #[must_use]
    pub fn new(payload: PushPayload) -> Self {
        let urgency = payload.event.urgency();
        let ttl = payload.event.ttl();
        Self {
            payload,
            urgency,
            ttl,
        }
    }
}

/// Which notifications a device receives, as an object with one boolean per
/// event a device can turn on or off:
///
/// - `inbox_item`: an agent filed an item in the user inbox
/// - `agent_failed`: an agent failed to start or crashed
/// - `outbound_unreachable`: a task sent to a remote agent can't reach it
/// - `reply_while_away`: an agent replied while no Residuum window was open
///
/// A new device receives `inbox_item` and `agent_failed`.
#[derive(Debug, Clone, PartialEq, Eq, TS)]
#[ts(
    export,
    type = "{ inbox_item: boolean, agent_failed: boolean, outbound_unreachable: boolean, reply_while_away: boolean }"
)]
pub struct PushPreferences {
    enabled: BTreeSet<PushEvent>,
}

impl Default for PushPreferences {
    fn default() -> Self {
        Self {
            enabled: BTreeSet::from([PushEvent::InboxItem, PushEvent::AgentFailed]),
        }
    }
}

impl PushPreferences {
    /// Whether the device wants notifications for `event`. The test
    /// notification goes out whatever the preferences say.
    #[must_use]
    pub fn wants(&self, event: PushEvent) -> bool {
        event == PushEvent::Test || self.enabled.contains(&event)
    }

    /// These preferences with the events `patch` names switched on or off.
    #[must_use]
    pub fn patched(&self, patch: PushPreferencesPatch) -> Self {
        let mut enabled = self.enabled.clone();
        for (event, _name) in PushEvent::SWITCHABLE {
            match patch.setting(event) {
                Some(true) => {
                    enabled.insert(event);
                }
                Some(false) => {
                    enabled.remove(&event);
                }
                None => {}
            }
        }
        Self { enabled }
    }
}

impl Serialize for PushPreferences {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct as _;

        let mut object =
            serializer.serialize_struct("PushPreferences", PushEvent::SWITCHABLE.len())?;
        for (event, name) in PushEvent::SWITCHABLE {
            object.serialize_field(name, &self.enabled.contains(&event))?;
        }
        object.end()
    }
}

impl<'de> Deserialize<'de> for PushPreferences {
    /// A name the object leaves out keeps its default, and a name it doesn't
    /// know is ignored.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let stated = BTreeMap::<String, bool>::deserialize(deserializer)?;
        let defaults = Self::default();
        let enabled = PushEvent::SWITCHABLE
            .into_iter()
            .filter(|(event, name)| {
                stated
                    .get(*name)
                    .copied()
                    .unwrap_or_else(|| defaults.enabled.contains(event))
            })
            .map(|(event, _name)| event)
            .collect();
        Ok(Self { enabled })
    }
}

/// Some of a device's preferences, in a request. A field left out keeps its
/// current value.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, TS)]
#[ts(export)]
pub struct PushPreferencesPatch {
    /// New value for `inbox_item`.
    #[serde(default)]
    #[ts(optional)]
    pub inbox_item: Option<bool>,
    /// New value for `agent_failed`.
    #[serde(default)]
    #[ts(optional)]
    pub agent_failed: Option<bool>,
    /// New value for `outbound_unreachable`.
    #[serde(default)]
    #[ts(optional)]
    pub outbound_unreachable: Option<bool>,
    /// New value for `reply_while_away`.
    #[serde(default)]
    #[ts(optional)]
    pub reply_while_away: Option<bool>,
}

impl PushPreferencesPatch {
    /// What the patch sets for `event`, if it names it.
    fn setting(self, event: PushEvent) -> Option<bool> {
        match event {
            PushEvent::InboxItem => self.inbox_item,
            PushEvent::AgentFailed => self.agent_failed,
            PushEvent::OutboundUnreachable => self.outbound_unreachable,
            PushEvent::ReplyWhileAway => self.reply_while_away,
            PushEvent::Test => None,
        }
    }
}

/// The delivery that most recently failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PushFailure {
    /// When the failure was recorded.
    #[ts(type = "string")]
    pub at: DateTime<Utc>,
    /// The HTTP status the push service answered, or `null` when it was never
    /// reached (or nothing could be sent).
    pub status: Option<u16>,
    /// What went wrong, in plain words, and what to do about it.
    pub message: String,
}

/// One browser or installed app that receives notifications.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PushDevice {
    /// The hub's id for the device.
    pub id: String,
    /// The name the user gave it.
    pub label: String,
    /// When the device was first registered.
    #[ts(type = "string")]
    pub created_at: DateTime<Utc>,
    /// When a notification last reached the push service, or `null` if none
    /// has.
    #[ts(type = "string | null")]
    pub last_success_at: Option<DateTime<Utc>>,
    /// The most recent failure. It stays until the next failure replaces it,
    /// so compare its `at` with `last_success_at` to tell whether delivery
    /// has recovered.
    pub last_failure: Option<PushFailure>,
    /// Which notifications the device receives.
    pub preferences: PushPreferences,
}

impl PushDevice {
    /// Whether the device's latest delivery attempt failed: it has a failure
    /// that no success has followed.
    #[must_use]
    pub fn is_failing(&self) -> bool {
        self.last_failure
            .as_ref()
            .is_some_and(|failure| self.last_success_at.is_none_or(|ok| failure.at > ok))
    }
}

/// The keys of a browser's push subscription.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WebPushSubscriptionKeys {
    /// The browser's P-256 public key, base64url.
    pub p256dh: String,
    /// The subscription's 16-byte authentication secret, base64url.
    pub auth: String,
}

/// A browser's push subscription, as `PushSubscription.toJSON()` gives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WebPushSubscription {
    /// The push service's URL for this subscription. Whoever holds it can
    /// send to the device, so the API never returns it.
    pub endpoint: String,
    /// The keys notifications are encrypted with.
    pub keys: WebPushSubscriptionKeys,
}

/// Body of `PUT /api/hub/push/devices`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, TS)]
#[ts(export)]
pub struct PutPushDeviceRequest {
    /// The browser's subscription.
    pub subscription: WebPushSubscription,
    /// The device's name. A new device without one is called "Unnamed
    /// device"; an existing one keeps its name.
    #[serde(default)]
    #[ts(optional)]
    pub label: Option<String>,
    /// Preferences to set. A new device starts from the defaults; an existing
    /// one from its current preferences.
    #[serde(default)]
    #[ts(optional)]
    pub preferences: Option<PushPreferencesPatch>,
}

/// Body of `PATCH /api/hub/push/devices/{id}`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, TS)]
#[ts(export)]
pub struct PatchPushDeviceRequest {
    /// A new name.
    #[serde(default)]
    #[ts(optional)]
    pub label: Option<String>,
    /// Preferences to change.
    #[serde(default)]
    #[ts(optional)]
    pub preferences: Option<PushPreferencesPatch>,
}

/// Answer of `GET /api/hub/push/key`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct PushKeyResponse {
    /// The hub's VAPID public key, base64url: the `applicationServerKey` a
    /// browser subscribes with.
    pub public_key: String,
}

/// Answer of `GET /api/hub/push/devices`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct PushDeviceList {
    /// Every registered device, oldest first.
    pub devices: Vec<PushDevice>,
}

/// Answer of the routes that create or change one device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct PushDeviceResponse {
    /// The device as it is now.
    pub device: PushDevice,
}

/// Answer of `POST /api/hub/push/devices/{id}/test`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct PushTestResult {
    /// Whether the push service accepted the notification.
    pub delivered: bool,
    /// What went wrong, in plain words, when it wasn't accepted.
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_devices_start_with_inbox_and_failures_on() {
        let defaults = PushPreferences::default();
        assert!(defaults.wants(PushEvent::InboxItem) && defaults.wants(PushEvent::AgentFailed));
        assert!(
            !defaults.wants(PushEvent::OutboundUnreachable)
                && !defaults.wants(PushEvent::ReplyWhileAway)
        );
    }

    #[test]
    fn a_patch_replaces_only_the_fields_it_names() {
        let patch: PushPreferencesPatch =
            serde_json::from_str(r#"{ "reply_while_away": true, "inbox_item": false }"#).unwrap();
        let patched = PushPreferences::default().patched(patch);
        assert_eq!(
            serde_json::to_value(&patched).unwrap(),
            serde_json::json!({
                "inbox_item": false,
                "agent_failed": true,
                "outbound_unreachable": false,
                "reply_while_away": true,
            })
        );
    }

    #[test]
    fn preferences_are_an_object_of_booleans_on_the_wire() {
        let defaults = serde_json::to_value(PushPreferences::default()).unwrap();
        assert_eq!(
            defaults,
            serde_json::json!({
                "inbox_item": true,
                "agent_failed": true,
                "outbound_unreachable": false,
                "reply_while_away": false,
            })
        );
        let read: PushPreferences = serde_json::from_value(defaults).unwrap();
        assert_eq!(read, PushPreferences::default());

        let partial: PushPreferences =
            serde_json::from_str(r#"{ "agent_failed": false, "unknown": true }"#).unwrap();
        assert!(!partial.wants(PushEvent::AgentFailed));
        assert!(
            partial.wants(PushEvent::InboxItem),
            "a name left out keeps its default"
        );
    }

    #[test]
    fn the_test_notification_ignores_preferences() {
        let all_off: PushPreferencesPatch = serde_json::from_str(
            r#"{ "inbox_item": false, "agent_failed": false, "outbound_unreachable": false, "reply_while_away": false }"#,
        )
        .unwrap();
        let none = PushPreferences::default().patched(all_off);
        assert!(none.wants(PushEvent::Test));
        assert!(!none.wants(PushEvent::InboxItem));
        assert!(!none.wants(PushEvent::AgentFailed));
    }

    #[test]
    fn events_carry_the_urgency_and_lifetime_of_the_design() {
        let hours = |event: PushEvent| event.ttl().as_secs() / 3600;
        assert_eq!(PushEvent::AgentFailed.urgency(), Urgency::High);
        assert_eq!(PushEvent::InboxItem.urgency(), Urgency::Normal);
        assert_eq!(hours(PushEvent::InboxItem), 24);
        assert_eq!(hours(PushEvent::AgentFailed), 24);
        assert_eq!(hours(PushEvent::OutboundUnreachable), 6);
        assert_eq!(hours(PushEvent::ReplyWhileAway), 1);
    }

    #[test]
    fn a_payload_serializes_with_the_documented_field_names() {
        let payload = PushPayload::new(
            PushEvent::InboxItem,
            "scout",
            "Weekly digest",
            "From scout: three new papers",
            "/inbox?item=scout:digest-1",
            "inbox:scout:digest-1",
            4,
        );
        assert_eq!(
            serde_json::to_value(&payload).unwrap(),
            serde_json::json!({
                "v": 1,
                "event": "inbox_item",
                "agent": "scout",
                "title": "Weekly digest",
                "body": "From scout: three new papers",
                "target": "/inbox?item=scout:digest-1",
                "tag": "inbox:scout:digest-1",
                "badge": 4,
            })
        );
    }

    #[test]
    fn a_long_body_is_cut_to_the_limit_with_an_ellipsis() {
        let long = "word ".repeat(60);
        let payload = PushPayload::new(PushEvent::ReplyWhileAway, "scout", "t", &long, "/", "x", 0);
        assert!(payload.body.chars().count() <= MAX_BODY_CHARS);
        assert!(payload.body.ends_with('…'));

        let short = PushPayload::new(PushEvent::ReplyWhileAway, "scout", "t", "hi", "/", "x", 0);
        assert_eq!(short.body, "hi");
    }

    #[test]
    fn truncation_never_splits_a_multibyte_character() {
        let text = "é".repeat(200);
        let cut = truncate_chars(&text, MAX_BODY_CHARS);
        assert_eq!(cut.chars().count(), MAX_BODY_CHARS);
    }

    #[test]
    fn a_device_is_failing_while_its_latest_failure_has_no_success_after_it() {
        let at = |secs: i64| DateTime::from_timestamp(1_790_000_000 + secs, 0).unwrap();
        let device = |last_success_at: Option<i64>, failed_at: Option<i64>| PushDevice {
            id: "d".to_string(),
            label: "Laptop".to_string(),
            created_at: at(0),
            last_success_at: last_success_at.map(at),
            last_failure: failed_at.map(|secs| PushFailure {
                at: at(secs),
                status: Some(400),
                message: "no".to_string(),
            }),
            preferences: PushPreferences::default(),
        };

        assert!(!device(None, None).is_failing(), "never tried");
        assert!(!device(Some(10), None).is_failing(), "only successes");
        assert!(device(None, Some(10)).is_failing(), "only failures");
        assert!(device(Some(10), Some(20)).is_failing(), "failed since");
        assert!(
            !device(Some(30), Some(20)).is_failing(),
            "delivery recovered after the failure"
        );
    }
}
