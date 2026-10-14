//! Tunnel v2 control frames, sent as JSON text WebSocket messages. Stream data
//! travels separately as binary messages (see [`super::stream`]). The shapes
//! mirror the relay's `V2Frame`.

use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The host names the relay claims this instance answers for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WireHosts {
    pub ui: String,
    pub workbench: String,
    pub instance: String,
}

/// One agent this hub advertises to the relay in a [`V2Frame::AgentsUpdate`]
/// frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AgentInfo {
    /// The agent's name: its identity and its A2A path segment.
    pub name: String,
    /// A human-readable label for the agent, shown in the relay's A2A
    /// directory.
    pub display_name: String,
    /// Whether the agent answers A2A requests right now.
    pub a2a_enabled: bool,
    /// Whether the agent is gated by a caller key, rather than open to anyone.
    pub a2a_private: bool,
}

/// One of the user's instances, as listed in [`V2Frame::InstancesUpdate`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct InstanceSummary {
    pub slug: String,
    pub display_name: String,
    pub active: bool,
    pub connected: bool,
}

/// A control frame on a v2 tunnel.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum V2Frame {
    /// Relay to instance, first frame after registration.
    Connected {
        user: String,
        instance: String,
        keepalive_interval_secs: u64,
        hosts: WireHosts,
        directory_url: String,
        a2a_token: String,
    },
    /// Relay to instance keepalive.
    Ping,
    /// Instance reply to [`Self::Ping`].
    Pong,
    /// Instance to relay: the full list of the instance's agents.
    AgentsUpdate { agents: Vec<AgentInfo> },
    /// Relay to instance: a browser connection for `host` has arrived.
    StreamOpen {
        stream_id: Uuid,
        host: String,
        peer_ip: String,
    },
    /// Either side: the stream is finished.
    StreamClose {
        stream_id: Uuid,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// Either side: grant the other more send window on the stream.
    StreamCredit { stream_id: Uuid, bytes: u64 },
    /// Instance to relay: route TLS-ALPN-01 validation for `names` here.
    ChallengeClaim { names: Vec<String> },
    /// Instance to relay: stop routing validation for `names` here.
    ChallengeRelease { names: Vec<String> },
    /// Relay to instance: the claim is held.
    ChallengeGranted { names: Vec<String> },
    /// Relay to instance: another instance of the user holds the claim.
    ChallengeBusy { names: Vec<String> },
    /// Relay to instance: the user's instances, on connect and on change.
    InstancesUpdate { instances: Vec<InstanceSummary> },
    /// Instance to relay: make `slug` the user's active instance.
    ActivateInstance { slug: String },
    /// Instance to relay: ask for a pin-service grant.
    PinGrantRequest { purpose: String },
    /// Relay to instance: the grant for `purpose`, a compact JWS.
    PinGrant { purpose: String, grant: String },
    /// Relay to instance: no grant was issued.
    PinGrantError { purpose: String, reason: String },
}

/// `Debug` that never prints the relay-minted a2a token or a pin grant.
impl fmt::Debug for V2Frame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connected {
                user,
                instance,
                keepalive_interval_secs,
                hosts,
                directory_url,
                ..
            } => f
                .debug_struct("Connected")
                .field("user", user)
                .field("instance", instance)
                .field("keepalive_interval_secs", keepalive_interval_secs)
                .field("hosts", hosts)
                .field("directory_url", directory_url)
                .field("a2a_token", &"<redacted>")
                .finish(),
            Self::PinGrant { purpose, .. } => f
                .debug_struct("PinGrant")
                .field("purpose", purpose)
                .field("grant", &"<redacted>")
                .finish(),
            Self::Ping => write!(f, "Ping"),
            Self::Pong => write!(f, "Pong"),
            Self::AgentsUpdate { agents } => f
                .debug_struct("AgentsUpdate")
                .field("agents", &agents.len())
                .finish(),
            Self::StreamOpen {
                stream_id,
                host,
                peer_ip,
            } => f
                .debug_struct("StreamOpen")
                .field("stream_id", stream_id)
                .field("host", host)
                .field("peer_ip", peer_ip)
                .finish(),
            Self::StreamClose { stream_id, reason } => f
                .debug_struct("StreamClose")
                .field("stream_id", stream_id)
                .field("reason", reason)
                .finish(),
            Self::StreamCredit { stream_id, bytes } => f
                .debug_struct("StreamCredit")
                .field("stream_id", stream_id)
                .field("bytes", bytes)
                .finish(),
            Self::ChallengeClaim { names } => f
                .debug_struct("ChallengeClaim")
                .field("names", names)
                .finish(),
            Self::ChallengeRelease { names } => f
                .debug_struct("ChallengeRelease")
                .field("names", names)
                .finish(),
            Self::ChallengeGranted { names } => f
                .debug_struct("ChallengeGranted")
                .field("names", names)
                .finish(),
            Self::ChallengeBusy { names } => f
                .debug_struct("ChallengeBusy")
                .field("names", names)
                .finish(),
            Self::InstancesUpdate { instances } => f
                .debug_struct("InstancesUpdate")
                .field("instances", instances)
                .finish(),
            Self::ActivateInstance { slug } => f
                .debug_struct("ActivateInstance")
                .field("slug", slug)
                .finish(),
            Self::PinGrantRequest { purpose } => f
                .debug_struct("PinGrantRequest")
                .field("purpose", purpose)
                .finish(),
            Self::PinGrantError { purpose, reason } => f
                .debug_struct("PinGrantError")
                .field("purpose", purpose)
                .field("reason", reason)
                .finish(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_use_snake_case_tags() {
        let json = serde_json::to_string(&V2Frame::ChallengeBusy {
            names: vec!["a.example".into()],
        })
        .unwrap();
        assert_eq!(json, r#"{"type":"challenge_busy","names":["a.example"]}"#);
    }

    #[test]
    fn stream_close_reason_is_optional() {
        let id = Uuid::new_v4();
        let bare = serde_json::to_string(&V2Frame::StreamClose {
            stream_id: id,
            reason: None,
        })
        .unwrap();
        assert!(!bare.contains("reason"), "{bare}");
        let parsed: V2Frame =
            serde_json::from_str(&format!(r#"{{"type":"stream_close","stream_id":"{id}"}}"#))
                .unwrap();
        assert!(matches!(
            parsed,
            V2Frame::StreamClose { stream_id, reason: None } if stream_id == id
        ));
    }

    #[test]
    fn connected_parses_the_relays_json() {
        let json = r#"{"type":"connected","user":"bear","instance":"laptop","keepalive_interval_secs":30,"hosts":{"ui":"bear.example.com","workbench":"bear.workbench.example.com","instance":"laptop.bear.example.com"},"directory_url":"https://example.com/a2a/bear/agents","a2a_token":"rsa_secret"}"#;
        let frame: V2Frame = serde_json::from_str(json).unwrap();
        let V2Frame::Connected {
            user,
            keepalive_interval_secs,
            hosts,
            ..
        } = &frame
        else {
            panic!("expected connected");
        };
        assert_eq!((user.as_str(), *keepalive_interval_secs), ("bear", 30));
        assert_eq!(hosts.workbench, "bear.workbench.example.com");
        assert_eq!(serde_json::to_string(&frame).unwrap(), json);
    }

    #[test]
    fn control_frames_serialize_to_the_relays_exact_json() {
        let id = Uuid::nil();
        let cases = [
            (V2Frame::Ping, r#"{"type":"ping"}"#.to_string()),
            (V2Frame::Pong, r#"{"type":"pong"}"#.to_string()),
            (
                V2Frame::StreamOpen {
                    stream_id: id,
                    host: "h".into(),
                    peer_ip: "1.2.3.4".into(),
                },
                format!(
                    r#"{{"type":"stream_open","stream_id":"{id}","host":"h","peer_ip":"1.2.3.4"}}"#
                ),
            ),
            (
                V2Frame::StreamCredit {
                    stream_id: id,
                    bytes: 5,
                },
                format!(r#"{{"type":"stream_credit","stream_id":"{id}","bytes":5}}"#),
            ),
            (
                V2Frame::ChallengeClaim {
                    names: vec!["a".into()],
                },
                r#"{"type":"challenge_claim","names":["a"]}"#.to_string(),
            ),
            (
                V2Frame::ActivateInstance { slug: "s".into() },
                r#"{"type":"activate_instance","slug":"s"}"#.to_string(),
            ),
            (
                V2Frame::PinGrantRequest {
                    purpose: "enroll".into(),
                },
                r#"{"type":"pin_grant_request","purpose":"enroll"}"#.to_string(),
            ),
            (
                V2Frame::PinGrant {
                    purpose: "enroll".into(),
                    grant: "g".into(),
                },
                r#"{"type":"pin_grant","purpose":"enroll","grant":"g"}"#.to_string(),
            ),
            (
                V2Frame::PinGrantError {
                    purpose: "reset".into(),
                    reason: "no".into(),
                },
                r#"{"type":"pin_grant_error","purpose":"reset","reason":"no"}"#.to_string(),
            ),
            (
                V2Frame::InstancesUpdate {
                    instances: vec![InstanceSummary {
                        slug: "a".into(),
                        display_name: "A".into(),
                        active: true,
                        connected: false,
                    }],
                },
                r#"{"type":"instances_update","instances":[{"slug":"a","display_name":"A","active":true,"connected":false}]}"#.to_string(),
            ),
        ];
        for (frame, json) in cases {
            assert_eq!(serde_json::to_string(&frame).unwrap(), json);
            let parsed: V2Frame = serde_json::from_str(&json).unwrap();
            assert_eq!(serde_json::to_string(&parsed).unwrap(), json);
        }
    }

    #[test]
    fn debug_never_prints_secrets() {
        let connected = V2Frame::Connected {
            user: "bear".into(),
            instance: "laptop".into(),
            keepalive_interval_secs: 30,
            hosts: WireHosts {
                ui: "u".into(),
                workbench: "w".into(),
                instance: "i".into(),
            },
            directory_url: "d".into(),
            a2a_token: "rsa_secret".into(),
        };
        let grant = V2Frame::PinGrant {
            purpose: "enroll".into(),
            grant: "jws.secret.grant".into(),
        };
        let text = format!("{connected:?} {grant:?}");
        assert!(!text.contains("rsa_secret") && !text.contains("jws.secret.grant"));
    }

    #[test]
    fn unknown_frame_types_fail_to_parse() {
        assert!(serde_json::from_str::<V2Frame>(r#"{"type":"nope"}"#).is_err());
    }
}
