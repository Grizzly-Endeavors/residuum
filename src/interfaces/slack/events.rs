//! Serde types for the Slack wire formats the adapter consumes.
//!
//! Contract discipline: the shapes here are authored from Slack's published
//! documentation and validated against the fixtures in [`fixtures/`] — which
//! are **assumed-until-probed** and replaced with captured payloads at first
//! live deployment. Build against the fixtures, never against memory.
//!
//! Tolerant-unknowns rule (design D4): an envelope type Slack adds later
//! deserializes to [`EnvelopeKind::Unknown`] carrying the type string, is
//! logged, skipped, and still acknowledged — never an error.

use serde::Deserialize;
use serde_json::Value;

/// The Socket Mode envelope wrapper every event arrives in.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Envelope {
    /// Unique id for this delivery; redeliveries reuse it (dedup key).
    #[serde(default, rename = "envelope_id")]
    pub(crate) id: String,
    /// What the envelope carries.
    #[serde(rename = "type")]
    pub(crate) kind: EnvelopeKind,
    /// Type-specific payload; `None` on `hello`.
    #[serde(default)]
    pub(crate) payload: Option<Value>,
    /// Where the ack is sent via HTTP `POST`; empty on envelopes that need no ack.
    #[serde(default)]
    pub(crate) response_url: String,
}

impl Envelope {
    /// The inner Events API event object, if this envelope carries one.
    pub(crate) fn event(&self) -> Option<&Value> {
        self.payload.as_ref()?.get("event")
    }
}

/// Envelope kinds the adapter distinguishes. Everything else is
/// [`EnvelopeKind::Unknown`] — tolerated, logged by type, never fatal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EnvelopeKind {
    Hello,
    EventsApi,
    Disconnect,
    /// An unrecognized `type` string, preserved for logging.
    Unknown(String),
}

impl<'de> Deserialize<'de> for EnvelopeKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Ok(match raw.as_str() {
            "hello" => EnvelopeKind::Hello,
            "events_api" => EnvelopeKind::EventsApi,
            "disconnect" => EnvelopeKind::Disconnect,
            other => EnvelopeKind::Unknown(other.to_string()),
        })
    }
}

/// A plain, user-authored DM message — the only inbound event v1 acts on.
///
/// Structural filtering (DM-ness, plainness) lives in [`DmMessageEvent::parse`];
/// policy filtering (owner gate, bot exclusion) lives in the handler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DmMessageEvent {
    /// DM channel id — also the reply target.
    pub(crate) channel: String,
    /// Authoring user id.
    pub(crate) user: String,
    /// Message text.
    pub(crate) text: String,
    /// Message timestamp (Slack ts string).
    pub(crate) ts: String,
    /// Set when a bot (ours or another) authored the message; the handler's
    /// self/bot guard keys on this.
    pub(crate) bot_id: Option<String>,
}

impl DmMessageEvent {
    /// Parse an Events API event object; `Some` only for a plain message in a
    /// DM. Subtyped events (edits, deletions, joins, bot system messages) and
    /// non-DM messages return `None` — logged and skipped, per design D1/D4.
    pub(crate) fn parse(event: &Value) -> Option<Self> {
        if event.get("type")?.as_str()? != "message" {
            return None;
        }
        if event.get("channel_type")?.as_str()? != "im" {
            return None;
        }
        // v1 acts only on plain messages; subtypes (message_changed, …) are skipped.
        if event.get("subtype").and_then(Value::as_str).is_some() {
            return None;
        }
        Some(Self {
            channel: event.get("channel")?.as_str()?.to_string(),
            user: event.get("user")?.as_str()?.to_string(),
            text: event.get("text")?.as_str()?.to_string(),
            ts: event.get("ts")?.as_str()?.to_string(),
            bot_id: event
                .get("bot_id")
                .and_then(Value::as_str)
                .map(str::to_string),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_round_trips_from_fixtures() {
        let hello: Envelope =
            serde_json::from_str(include_str!("fixtures/envelope-hello.json")).unwrap();
        assert_eq!(hello.kind, EnvelopeKind::Hello);
        assert!(hello.payload.is_none());

        let events: Envelope =
            serde_json::from_str(include_str!("fixtures/envelope-events-api-message.json"))
                .unwrap();
        assert_eq!(events.kind, EnvelopeKind::EventsApi);
        assert_eq!(events.id, "env-evt-1");
        assert_eq!(
            events.response_url,
            "https://hooks.slack.com/response/abc123"
        );

        let disconnect: Envelope =
            serde_json::from_str(include_str!("fixtures/envelope-disconnect.json")).unwrap();
        assert_eq!(disconnect.kind, EnvelopeKind::Disconnect);
    }

    #[test]
    fn unknown_envelope_type_is_tolerated_not_errored() {
        let raw = include_str!("fixtures/envelope-unknown.json");
        let env: Envelope = serde_json::from_str(raw).unwrap();
        assert_eq!(env.kind, EnvelopeKind::Unknown("slash_commands".into()));
    }

    #[test]
    fn events_api_fixture_yields_dm_message_end_to_end() {
        let env: Envelope =
            serde_json::from_str(include_str!("fixtures/envelope-events-api-message.json"))
                .unwrap();
        let ev = DmMessageEvent::parse(env.event().expect("event payload")).expect("dm message");
        assert_eq!(ev.text, "hello from slack");
        assert_eq!(ev.channel, "D0001");
        assert_eq!(ev.user, "U0001");
        assert!(ev.bot_id.is_none());
    }

    #[test]
    fn dm_message_event_filters_non_dm() {
        let not_dm = serde_json::json!({
            "type": "message", "channel_type": "channel",
            "channel": "C0001", "user": "U0001", "text": "hi", "ts": "1"
        });
        assert!(DmMessageEvent::parse(&not_dm).is_none());
    }

    #[test]
    fn dm_message_event_filters_subtyped() {
        let edited = serde_json::json!({
            "type": "message", "channel_type": "im", "subtype": "message_changed",
            "channel": "D0001", "user": "U0001", "text": "hi", "ts": "1"
        });
        assert!(DmMessageEvent::parse(&edited).is_none());
    }

    #[test]
    fn dm_message_event_filters_non_message() {
        let reaction = serde_json::json!({
            "type": "reaction_added", "channel_type": "im",
            "channel": "D0001", "user": "U0001", "ts": "1"
        });
        assert!(DmMessageEvent::parse(&reaction).is_none());
    }

    #[test]
    fn bot_authored_dm_message_parses_for_handler_policy() {
        // Structural parse keeps bot-authored messages; the handler's policy
        // gate rejects them — split verified by this pair.
        let bot_msg = serde_json::json!({
            "type": "message", "channel_type": "im", "bot_id": "B061",
            "channel": "D0001", "user": "U0001", "text": "beep", "ts": "1"
        });
        let ev = DmMessageEvent::parse(&bot_msg).expect("parses structurally");
        assert_eq!(ev.bot_id.as_deref(), Some("B061"));
    }
}
