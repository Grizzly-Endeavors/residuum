//! The shapes of the Web Push HTTP API and of the payload the service worker
//! receives. Field names and JSON forms follow
//! `docs/systems-usage/hub-http.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PushEvent {
    InboxItem,
    AgentFailed,
    OutboundUnreachable,
    ReplyWhileAway,
    Test,
}

impl PushEvent {
    const SWITCHABLE: [(Self, &'static str); 4] = [
        (Self::InboxItem, "inbox_item"),
        (Self::AgentFailed, "agent_failed"),
        (Self::OutboundUnreachable, "outbound_unreachable"),
        (Self::ReplyWhileAway, "reply_while_away"),
    ];

    #[must_use]
    pub fn urgency(self) -> Urgency {
        match self {
            Self::AgentFailed => Urgency::High,
            Self::InboxItem | Self::OutboundUnreachable | Self::ReplyWhileAway | Self::Test => {
                Urgency::Normal
            }
        }
    }

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Urgency {
    High,
    Normal,
}

impl Urgency {
    #[must_use]
    pub(super) fn header_value(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Normal => "normal",
        }
    }
}

pub const MAX_BODY_CHARS: usize = 120;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PushPayload {
    #[ts(type = "1")]
    pub v: u8,
    pub event: PushEvent,
    pub agent: String,
    pub title: String,
    pub body: String,
    pub target: String,
    pub tag: String,
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

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let kept: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", kept.trim_end())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushMessage {
    pub payload: PushPayload,
    pub urgency: Urgency,
    pub ttl: Duration,
}

impl PushMessage {
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
    #[must_use]
    pub fn wants(&self, event: PushEvent) -> bool {
        event == PushEvent::Test || self.enabled.contains(&event)
    }

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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, TS)]
#[ts(export)]
pub struct PushPreferencesPatch {
    #[serde(default)]
    #[ts(optional)]
    pub inbox_item: Option<bool>,
    #[serde(default)]
    #[ts(optional)]
    pub agent_failed: Option<bool>,
    #[serde(default)]
    #[ts(optional)]
    pub outbound_unreachable: Option<bool>,
    #[serde(default)]
    #[ts(optional)]
    pub reply_while_away: Option<bool>,
}

impl PushPreferencesPatch {
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PushFailure {
    #[ts(type = "string")]
    pub at: DateTime<Utc>,
    pub status: Option<u16>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PushDevice {
    pub id: String,
    pub label: String,
    #[ts(type = "string")]
    pub created_at: DateTime<Utc>,
    #[ts(type = "string | null")]
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_failure: Option<PushFailure>,
    pub preferences: PushPreferences,
}

impl PushDevice {
    #[must_use]
    pub fn is_failing(&self) -> bool {
        self.last_failure
            .as_ref()
            .is_some_and(|failure| self.last_success_at.is_none_or(|ok| failure.at > ok))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WebPushSubscriptionKeys {
    pub p256dh: String,
    pub auth: String,
}

/// A browser's push subscription, as `PushSubscription.toJSON()` gives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WebPushSubscription {
    /// The push service's URL for this subscription. Whoever holds it can
    /// send to the device, so the API never returns it.
    pub endpoint: String,
    pub keys: WebPushSubscriptionKeys,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, TS)]
#[ts(export)]
pub struct PutPushDeviceRequest {
    pub subscription: WebPushSubscription,
    #[serde(default)]
    #[ts(optional)]
    pub label: Option<String>,
    #[serde(default)]
    #[ts(optional)]
    pub preferences: Option<PushPreferencesPatch>,
    #[serde(default)]
    #[ts(optional)]
    pub previous_endpoint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, TS)]
#[ts(export)]
pub struct PatchPushDeviceRequest {
    #[serde(default)]
    #[ts(optional)]
    pub label: Option<String>,
    #[serde(default)]
    #[ts(optional)]
    pub preferences: Option<PushPreferencesPatch>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct PushKeyResponse {
    pub public_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct PushDeviceList {
    pub devices: Vec<PushDevice>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct PushDeviceResponse {
    pub device: PushDevice,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct PushTestResult {
    pub delivered: bool,
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
            "/inbox?thread=scout%2Fuser%2Fnote-1",
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
                "target": "/inbox?thread=scout%2Fuser%2Fnote-1",
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
